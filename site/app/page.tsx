import Architecture from "./components/Architecture";
import CopyChip from "./components/CopyChip";
import Icon from "./components/Icon";
import Ledger from "./components/Ledger";

const REPO = "https://github.com/dvansari65/mandate_ledger";
const DOCS = `${REPO}/blob/main/docs`;

export default function Page() {
  return (
    <>
      <header className="nav-bar">
        <div className="wrap nav">
          <a href="#" className="brand" aria-label="mandate-ledger home">
            <span className="mark" aria-hidden="true">
              <svg viewBox="0 0 32 32" width="22" height="22"><rect width="32" height="32" rx="7" fill="currentColor" /><rect x="7" y="9" width="18" height="2.5" rx="1" fill="var(--paper)" /><rect x="7" y="15" width="18" height="2.5" rx="1" fill="var(--paper)" /><rect x="7" y="21" width="11" height="2.5" rx="1" fill="var(--paper)" /><rect x="21" y="20" width="4" height="4.5" rx="1" fill="var(--red)" /></svg>
            </span>
            mandate-ledger
          </a>
          <nav className="nav-links" aria-label="Sections">
            <a href="#architecture">Where it sits</a>
            <a href="#failures">What breaks</a>
            <a href="#calls">The API</a>
            <a href="#who">Integrate</a>
          </nav>
          <div className="nav-actions">
            <a className="btn ghost sm" href={REPO}><Icon name="star" size={15} />GitHub</a>
            <a className="btn primary sm" href={`${DOCS}/integration.md`}>Get started</a>
          </div>
        </div>
      </header>

      <main>
        {/* ── Hero ── */}
        <section className="hero">
          <div className="wrap hero-grid">
            <div className="hero-copy">
              <span className="pill"><i className="dot" aria-hidden="true" />v0.0.1 · Rust · Apache-2.0</span>
              <h1>Five checks that <em>never talk</em> to each other.</h1>
              <p className="lede">
                An agent’s payment is verified by five parties, none of which checks the others. mandate-ledger sits
                between them — it refuses the step when they disagree, and writes down why.
              </p>
              <div className="actions">
                <a className="btn primary" href={`${DOCS}/integration.md`}>Get started<Icon name="arrow" size={16} /></a>
                <CopyChip text={`cargo add ml-core --git ${REPO}`} />
              </div>
              <ul className="trust" aria-label="Verification facts">
                <li><b>48</b> tests</li>
                <li><b>6,000</b> property cases</li>
                <li><b>0</b> unsafe blocks</li>
                <li><b>24</b> stable deny codes</li>
              </ul>
            </div>
            <div className="hero-visual">
              <Ledger />
            </div>
          </div>
        </section>

        {/* ── Protocols ── */}
        <section className="wrap protocols" aria-label="Protocols covered">
          <span className="protocols-label">Built against the formal analysis of</span>
          <ul>
            <li>x402<small>Coinbase</small></li>
            <li>MPP<small>Stripe</small></li>
            <li>AP2<small>Google</small></li>
            <li>ACP<small>OpenAI · Stripe</small></li>
            <li><a href="https://arxiv.org/abs/2609.00060">arXiv 2609.00060<small>40 issues · 529 lemmas</small></a></li>
          </ul>
        </section>

        {/* ── Architecture ── */}
        <section className="wrap section" id="architecture">
          <div className="section-head reveal">
            <span className="eyebrow">Where it sits</span>
            <h2>Between the agent and the money. Never holding either.</h2>
            <p>Inputs come in from the wallet and the agent. Proofs come up from the rail. The ledger decides, records, and hands the merchant a token it can’t forge.</p>
          </div>
          <div className="window reveal">
            <div className="window-bar"><span className="dots" aria-hidden="true"><i /><i /><i /></span><span>architecture · one loop, 10 s · a second cart is refused at the scope check</span></div>
            <Architecture />
          </div>
          <div className="cards three reveal">
            <div className="card">
              <span className="icon"><Icon name="shield" /></span>
              <h3>Gate</h3>
              <p>Signature and signer trust, then scope, then budget — the last one atomic with the write, so two parallel requests can’t both squeeze through.</p>
            </div>
            <div className="card">
              <span className="icon"><Icon name="layers" /></span>
              <h3>Lifecycle</h3>
              <p>Each stage is a token the next one requires. Wrong order doesn’t compile. Every transition, and every refusal, is appended to the hash chain.</p>
            </div>
            <div className="card">
              <span className="icon"><Icon name="bank" /></span>
              <h3>Rail</h3>
              <p>The rail says what “final” means — captured, N confirmations. The ledger waits for it; the merchant is never shown anything before Settled.</p>
            </div>
          </div>
        </section>

        {/* ── Failures ── */}
        <section className="wrap section" id="failures">
          <div className="section-head reveal">
            <span className="eyebrow">What goes wrong today</span>
            <h2>Every step is locally valid. Nobody checks that they refer to the same payment.</h2>
            <p>The same five failures show up across x402, MPP, AP2 and ACP — the primitive exists, the caller can skip it. Each one becomes a refusal here.</p>
          </div>
          <div className="bento reveal">
            <div className="card">
              <span className="icon"><Icon name="swap" /></span>
              <h3>The wrong cart gets paid</h3>
              <p>You approved ₹470 at one store. A bug or a hostile page turns it into ₹4,700 somewhere else. Every signature still verifies.</p>
              <code className="badge deny">CART_BINDING_MISMATCH</code>
            </div>
            <div className="card">
              <span className="icon"><Icon name="repeat" /></span>
              <h3>A retry charges twice</h3>
              <p>The response is lost, the agent tries again, the merchant processes it again. Two charges, one refund ticket.</p>
              <code className="badge ok">same context · one charge</code>
            </div>
            <div className="card">
              <span className="icon"><Icon name="box" /></span>
              <h3>Goods ship before the money is final</h3>
              <p>The merchant sees a pending payment and delivers. The payment is dropped or reorganised away. 15 of 15 x402 facilitators tested had this gap.</p>
              <code className="badge ok">deliver requires Settled</code>
            </div>
            <div className="card">
              <span className="icon"><Icon name="stop" /></span>
              <h3>There is no stop button</h3>
              <p>You granted ₹8,000 a month. On day three you change your mind. AP2 has no in-protocol revocation — the mandate lives until it expires.</p>
              <code className="badge deny">MANDATE_REVOKED</code>
            </div>
            <div className="card">
              <span className="icon"><Icon name="layers" /></span>
              <h3>Parallel work overspends</h3>
              <p>Three sub-tasks each read “₹8,000 left” in the same instant. Each spends it. Here the budget is reserved inside the store’s append, so only one gets through.</p>
              <code className="badge deny">SCOPE_TOTAL_EXCEEDED</code>
            </div>
          </div>
        </section>

        {/* ── API ── */}
        <section className="wrap section" id="calls">
          <div className="split">
            <div className="reveal">
              <div className="section-head tight">
                <span className="eyebrow">The API</span>
                <h2>Five calls. Each returns the token the next requires.</h2>
                <p>Call them out of order and it doesn’t compile. Call them across requests and <code>resume(ctx)</code> hands the token back.</p>
              </div>
              <ol className="steps">
                <li><b>authorize</b><span>signature, trust, revocation, scope, cap, velocity, budget — reserves the amount</span></li>
                <li><b>record_payment</b><span>amount must match; proof must bind to this context or cart; nonce consumed once</span></li>
                <li><b>record_settlement</b><span>final, pending, or failed — the rail decides; failure releases the budget</span></li>
                <li><b>record_delivery</b><span>accepts only a <code>Settled</code>; there is no other way to make one</span></li>
                <li><b>evidence</b><span>the whole chain, refusals included; verifiable without your database</span></li>
              </ol>
            </div>
            <div className="window reveal">
              <div className="window-bar"><span className="dots" aria-hidden="true"><i /><i /><i /></span><span>checkout.rs</span></div>
              <pre className="code"><code>{`
`}<span className="k">let</span> auth    = ledger.<span className="f">authorize</span>(&amp;mandate, &amp;cart, <span className="s">"order-1"</span>)?;{`
`}<span className="k">let</span> paid    = ledger.<span className="f">record_payment</span>(&amp;auth, &amp;proof)?;{`
`}<span className="k">let</span> settled = <span className="k">match</span> ledger.<span className="f">record_settlement</span>(&amp;paid, &amp;finality)? {"{"}{`
`}    <span className="t">Settlement</span>::<span className="t">Settled</span>(s)   =&gt; s,{`
`}    <span className="t">Settlement</span>::<span className="t">Pending</span> {"{ .. }"} =&gt; <span className="k">return</span> <span className="f">retry_later</span>(),{`
`}    <span className="t">Settlement</span>::<span className="t">Failed</span>(f)    =&gt; <span className="k">return</span> ledger.<span className="f">compensate</span>(&amp;f, <span className="t">None</span>),{`
`}{"}"};{`
`}ledger.<span className="f">record_delivery</span>(&amp;settled, receipt)?;{`
`}<span className="k">let</span> bundle  = ledger.<span className="f">evidence</span>(auth.<span className="f">ctx</span>())?;  <span className="c">// self-verifying</span>{`
`}</code></pre>
            </div>
          </div>
        </section>

        {/* ── Enforcement ── */}
        <section className="wrap section" id="enforced">
          <div className="section-head reveal">
            <span className="eyebrow">Enforced three times</span>
            <h2>The same rule at the type level, in the engine, and in the store.</h2>
            <p>A guarantee that lives in one place is a policy. One that lives in three is a property.</p>
          </div>
          <div className="split reveal">
            <div className="cards stack">
              <div className="card row">
                <span className="icon"><Icon name="braces" /></span>
                <div><h3>Types</h3><p><code>record_delivery</code> takes a <code>Settled</code>. Passing a <code>Paid</code> is a compile error.</p></div>
              </div>
              <div className="card row">
                <span className="icon"><Icon name="cpu" /></span>
                <div><h3>Engine</h3><p>Every call re-reads the stored state before it acts. Tokens are proofs, not permissions.</p></div>
              </div>
              <div className="card row">
                <span className="icon"><Icon name="database" /></span>
                <div><h3>Store</h3><p><code>append</code> rejects any transition the machine doesn’t allow — even a hand-built event.</p></div>
              </div>
            </div>
            <div className="window">
              <div className="window-bar"><span className="dots" aria-hidden="true"><i /><i /><i /></span><span>cargo build</span></div>
              <pre className="code"><code>{`
`}<span className="c">// A merchant tries to ship on a pending payment.</span>{`
`}<span className="k">let</span> paid = ledger.<span className="f">record_payment</span>(&amp;auth, &amp;proof)?;{`
`}ledger.<span className="f">record_delivery</span>(&amp;paid, receipt)?;{`

`}<span className="e">error[E0308]</span>: mismatched types{`
`}   <span className="c">--&gt; checkout.rs:42:28</span>{`
`}    | ledger.record_delivery(&amp;paid, receipt)?;{`
`}    |                        <span className="e">^^^^^</span> expected `&amp;<span className="t">Settled</span>`, found `&amp;<span className="t">Paid</span>`{`

`}<span className="c">// 16 threads, ₹1,000 budget, ₹100 each → exactly ten succeed.</span>{`
`}<span className="c">// 8 threads, one nonce → exactly one is paid.</span>{`
`}<span className="g">test result: ok. 48 passed; 0 failed</span>{`
`}</code></pre>
            </div>
          </div>
        </section>

        {/* ── Integrators ── */}
        <section className="wrap section" id="who">
          <div className="section-head reveal">
            <span className="eyebrow">Who plugs in</span>
            <h2>Nobody rewrites their payment code. They add two gates to it.</h2>
          </div>
          <div className="cards four reveal">
            <div className="card">
              <span className="icon"><Icon name="cpu" /></span>
              <h3>Agents</h3>
              <p>A hard cap on what the model can spend, whatever it decides. Prompt injection can still try; it can’t get past authorize.</p>
              <span className="writes">writes <b>one authorize call</b></span>
            </div>
            <div className="card">
              <span className="icon"><Icon name="store" /></span>
              <h3>Merchants</h3>
              <p>Never ship or return data before the money is final. Never charged twice by a retry.</p>
              <span className="writes">writes <b>a CartAdapter</b>, gates fulfilment on Settled</span>
            </div>
            <div className="card">
              <span className="icon"><Icon name="bank" /></span>
              <h3>Payment gateways</h3>
              <p>One lifecycle across every rail. Webhooks resume the context. Every agent payment leaves a dispute-ready record.</p>
              <span className="writes">writes <b>one Rail adapter</b> per rail</span>
            </div>
            <div className="card">
              <span className="icon"><Icon name="wallet" /></span>
              <h3>Wallets</h3>
              <p>Customers set limits, watch the budget drain, and hit stop. “The agent did it” disputes come with evidence.</p>
              <span className="writes">writes <b>mandate signing</b>, revoke, attenuate</span>
            </div>
          </div>
        </section>

        {/* ── Scope ── */}
        <section className="wrap section" id="scope">
          <div className="section-head reveal">
            <span className="eyebrow">Honest scope</span>
            <h2>What it refuses to guess.</h2>
            <p>A scope the cart can’t satisfy is a refusal, not a pass. And some problems belong to other layers.</p>
          </div>
          <div className="scope reveal">
            <ul className="yes" aria-label="Covered">
              <li><Icon name="check" size={16} />Cart approved ≠ cart paid</li>
              <li><Icon name="check" size={16} />Double charge on retry, nonce reuse across contexts</li>
              <li><Icon name="check" size={16} />Release before finality; no rollback after failed settlement</li>
              <li><Icon name="check" size={16} />Out-of-scope merchant, category, amount, currency, velocity</li>
              <li><Icon name="check" size={16} />Revoked, expired, forged, or untrusted mandates</li>
              <li><Icon name="check" size={16} />Budget races under concurrency</li>
              <li><Icon name="check" size={16} />Tampered or truncated evidence</li>
            </ul>
            <ul className="no" aria-label="Not covered">
              <li><Icon name="x" size={16} />Detecting prompt injection itself — only bounding its effect</li>
              <li><Icon name="x" size={16} />Agent identity — that’s Visa TAP, Mastercard Agent Pay, Tenuo</li>
              <li><Icon name="x" size={16} />Counterfeit storefronts inside an allowed scope</li>
              <li><Icon name="x" size={16} />Merchant non-delivery after settlement — needs escrow</li>
              <li><Icon name="x" size={16} />Chain-level asset theft, gas abuse</li>
            </ul>
          </div>
        </section>

        {/* ── CTA ── */}
        <section className="wrap section">
          <div className="cta reveal">
            <div>
              <h2>Put a gate in front of the money.</h2>
              <p>Core engine complete and tested. Protocol adapters and a SQL store are next. Read it, break it, tell us.</p>
            </div>
            <div className="actions">
              <a className="btn primary" href={REPO}>Read the code<Icon name="arrow" size={16} /></a>
              <a className="btn ghost" href={`${DOCS}/threat-model.md`}>Threat model</a>
            </div>
          </div>
        </section>
      </main>

      <footer className="footer">
        <div className="wrap footer-grid">
          <div className="footer-brand">
            <span className="brand"><span className="mark" aria-hidden="true"><svg viewBox="0 0 32 32" width="18" height="18"><rect width="32" height="32" rx="7" fill="currentColor" /><rect x="7" y="9" width="18" height="2.5" rx="1" fill="var(--paper)" /><rect x="7" y="15" width="18" height="2.5" rx="1" fill="var(--paper)" /><rect x="7" y="21" width="11" height="2.5" rx="1" fill="var(--paper)" /></svg></span>mandate-ledger</span>
            <p>The enforcement layer for agent-driven payments. Free, Apache-2.0.</p>
          </div>
          <div><h4>Project</h4><a href={REPO}>GitHub</a><a href={`${DOCS}/integration.md`}>Integration guide</a><a href={`${DOCS}/threat-model.md`}>Threat model</a></div>
          <div><h4>Research</h4><a href="https://arxiv.org/abs/2609.00060">Formal analysis of agent payment protocols</a><a href="https://arxiv.org/abs/2605.30998">Free-riding the agentic web (x402)</a><a href="https://arxiv.org/abs/2607.19545">Risks on emerging x402 payments</a></div>
          <div><h4>Status</h4><span>0.0.1 · core engine</span><span>Adapters, SQL store next</span><span>Not for production money yet</span></div>
        </div>
        <div className="wrap footer-bar"><span>© 2026 mandate-ledger contributors</span><span>Apache-2.0</span></div>
      </footer>
    </>
  );
}
