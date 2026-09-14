import Diagram from "./components/Diagram";
import Reveal from "./components/Reveal";

const REPO = "https://github.com/dvansari65/mandate_ledger";
const DOCS = `${REPO}/blob/main/docs`;
const PAPER = "https://arxiv.org/abs/2609.00060";

/** Three ledger lines; the last one short, with a mark where the refusal goes. */
const Mark = () => (
  <svg viewBox="0 0 32 32" aria-hidden="true">
    <rect width="32" height="32" rx="8" className="mark-plate" />
    <rect x="7" y="9" width="18" height="2.4" rx="1.2" className="mark-line" />
    <rect x="7" y="14.8" width="18" height="2.4" rx="1.2" className="mark-line" />
    <rect x="7" y="20.6" width="10" height="2.4" rx="1.2" className="mark-line" />
    <rect x="20" y="19.6" width="5" height="5" rx="1.4" className="mark-dot" />
  </svg>
);

export default function Page() {
  return (
    <>
      <header className="nav-wrap">
        <div className="wrap nav">
          <a href="#" className="brand"><Mark />mandate-ledger</a>
          <nav className="links" aria-label="Sections">
            <a href="#how">How it works</a>
            <a href="#faq">FAQ</a>
            <a href={`${DOCS}/integration.md`}>Docs</a>
          </nav>
          <a className="btn sm" href={REPO}>GitHub</a>
        </div>
      </header>

      <main>
        <section className="wrap hero">
          <div className="hero-copy">
            <span className="tag rise">Enforcement layer for agent-driven payments</span>
            <h1 className="rise" style={{ animationDelay: "80ms" }}>The agent can spend. The ledger decides whether it <em>may</em>.</h1>
            <p className="lede rise" style={{ animationDelay: "160ms" }}>
              It sits between the agent and the payment rail, refuses any step where the mandate, the cart, the payment
              and the settlement disagree, and keeps the record. It never moves money.
            </p>
            <div className="actions rise" style={{ animationDelay: "240ms" }}>
              <a className="btn" href={`${DOCS}/integration.md`}>Get started</a>
              <a className="link" href={REPO}>View on GitHub →</a>
            </div>
          </div>
          <div className="rise" style={{ animationDelay: "200ms" }}><Diagram /></div>
        </section>

        <section className="wrap proof" aria-label="Trust">
          <ul>
            <li><b>Open source</b> Apache-2.0</li>
            <li><b>48</b> tests</li>
            <li><b>6,000</b> property cases</li>
            <li><b>0</b> unsafe code</li>
            <li>Built on the <a href={PAPER}>formal analysis</a> of x402, AP2, ACP and MPP</li>
          </ul>
        </section>

        <section className="band" id="problem">
          <div className="wrap">
            <Reveal className="head">
              <span className="kicker">The problem</span>
              <h2>An agent’s payment is checked five times. Never once <em>against each other</em>.</h2>
            </Reveal>
            <Reveal delay={80}>
              <div className="three">
                <div><h3>The wrong cart gets paid</h3><p>You approved ₹470. A hostile page turns it into ₹4,700 somewhere else. Every signature still looks valid.</p></div>
                <div><h3>A retry charges twice</h3><p>The response is lost, the agent tries again, the merchant charges again.</p></div>
                <div><h3>Goods ship before the money is final</h3><p>A pending payment looks paid. Then it reverts — and the goods are gone.</p></div>
              </div>
            </Reveal>
          </div>
        </section>

        <section className="wrap section" id="features">
          <Reveal className="head">
            <span className="kicker">What you get</span>
            <h2>One layer that makes every check <em>agree</em>.</h2>
          </Reveal>
          <Reveal delay={80}>
            <div className="four">
              <div><h3>Refuses when the pieces disagree</h3><p>Mandate, cart, payment and settlement are bound to each other. If any one doesn’t match, the step doesn’t happen.</p></div>
              <div><h3>Budgets that hold under load</h3><p>Limits are reserved the moment a purchase is approved, so parallel agents can’t overspend — and a mandate can be revoked at any time.</p></div>
              <div><h3>Delivery only after the money is final</h3><p>A merchant can’t ship on a pending payment. Not by policy — the path doesn’t exist.</p></div>
              <div><h3>A record you can hand to anyone</h3><p>Every decision, including every refusal, is chained and verifiable without your database. Disputes come with evidence.</p></div>
            </div>
          </Reveal>
        </section>

        <section className="band" id="how">
          <div className="wrap">
            <Reveal className="head">
              <span className="kicker">How it works</span>
              <h2>Three steps, always in <em>order</em>.</h2>
            </Reveal>
            <Reveal delay={80}>
              <ol className="steps">
                <li><span className="num">1</span><h3>Grant</h3><p>The user sets limits once — which merchants, how much, for how long. The mandate is signed, and can be revoked.</p></li>
                <li><span className="num">2</span><h3>Check</h3><p>Every step of the payment is checked against the mandate and the step before it. Budget is reserved before any money moves.</p></li>
                <li><span className="num">3</span><h3>Record</h3><p>When everything agrees, the money moves and the merchant is told. Every decision is written to a tamper-evident ledger.</p></li>
              </ol>
            </Reveal>
          </div>
        </section>

        <section className="wrap section" id="faq">
          <Reveal className="head">
            <span className="kicker">Questions</span>
            <h2>Before you <em>read the code</em>.</h2>
          </Reveal>
          <Reveal delay={80}>
            <div className="faq">
              <details open>
                <summary>Does it move money?</summary>
                <p>No. It decides whether each step may proceed and records the decision. Your payment provider or chain moves the money.</p>
              </details>
              <details>
                <summary>What does it not do?</summary>
                <p>It doesn’t detect prompt injection, verify who an agent is, or protect you from a fraudulent merchant that sits inside an allowed scope. Those belong to other layers — the <a href={`${DOCS}/threat-model.md`}>threat model</a> says exactly which.</p>
              </details>
              <details>
                <summary>Which protocols does it work with?</summary>
                <p>It’s built against the formal analysis of x402, AP2, ACP and MPP. The core is protocol-agnostic; adapters for each are next.</p>
              </details>
              <details>
                <summary>Is it ready for production?</summary>
                <p>Not yet. The core engine is complete and tested. Protocol adapters, a SQL store and an external audit are in progress. Don’t put money behind it yet.</p>
              </details>
              <details>
                <summary>How do I plug it in?</summary>
                <p>Three small pieces — one for your cart format, one for your payment rail, one for storage — with in-memory versions to start. The <a href={`${DOCS}/integration.md`}>integration guide</a> walks through it.</p>
              </details>
            </div>
          </Reveal>
        </section>

        <section className="close">
          <div className="wrap">
            <Reveal>
              <h2>Put a gate in front of the <em>money</em>.</h2>
              <p>Read it, break it, tell us.</p>
              <div className="actions">
                <a className="btn dark" href={`${DOCS}/integration.md`}>Get started</a>
                <a className="link inv" href={`${DOCS}/threat-model.md`}>Read the threat model →</a>
              </div>
              <small>0.0.1 — core engine complete and tested. Not for production money yet.</small>
            </Reveal>
          </div>
        </section>
      </main>

      <footer className="wrap footer">
        <span className="brand"><Mark />mandate-ledger</span>
        <nav aria-label="Footer">
          <a href={REPO}>GitHub</a>
          <a href={`${DOCS}/integration.md`}>Docs</a>
          <a href={`${DOCS}/threat-model.md`}>Threat model</a>
          <a href={`${REPO}/blob/main/LICENSE`}>License</a>
        </nav>
        <span className="copy">© 2026 mandate-ledger contributors · Apache-2.0</span>
      </footer>
    </>
  );
}
