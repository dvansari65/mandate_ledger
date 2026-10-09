# shellcheck shell=bash
# Shared by every scenario script. Source it; do not run it.
#
# A scenario drives `ml` one process per step, as an operator or a service
# would: nothing is shared between two steps except the database and the
# files in the scenario's working directory. This file runs a step,
# captures what the shell sees — exit code, report, stderr — and asserts
# on it. A failed assertion prints the step, its report and its stderr,
# keeps the working directory, and ends the scenario with exit 1.
#
# Needs ML_DATABASE_URL, jq, and either `ML` naming an `ml` binary or a
# cargo toolchain to build one. Every id a scenario writes — principal,
# mandate, request key, payment reference — carries a suffix unique to the
# run, so a durable database can host any number of runs without one
# seeing another's budget or nonces.

set -euo pipefail

: "${ML_DATABASE_URL:?set ML_DATABASE_URL to the PostgreSQL the scenarios may write to}"
command -v jq >/dev/null || {
    echo "error: jq is required" >&2
    exit 1
}
if [ -z "${ML:-}" ]; then
    root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
    (cd "$root" && cargo build -q -p ml-sandbox)
    ML="${CARGO_TARGET_DIR:-$root/target}/debug/ml"
fi
[ -x "$ML" ] || {
    echo "error: $ML is not an executable" >&2
    exit 1
}

# ── The last step: what the shell saw ────────────────────────────────────

CODE=0
OUT=
ERR=
LAST=
CHECKS=0

# Run any command as its own process, capturing its exit code, stdout and
# stderr into CODE, OUT and ERR. Never aborts: the exit code is a fact for
# an assertion to look at.
capture() {
    LAST="$*"
    if "$@" >"$WORK/.out" 2>"$WORK/.err"; then
        CODE=0
    else
        CODE=$?
    fi
    OUT=$(cat "$WORK/.out")
    ERR=$(cat "$WORK/.err")
}

# Run `ml --json ARGS` as its own process.
step() {
    capture "$ML" --json "$@"
}

# Load what a process run in the background saw, from PREFIX.code,
# PREFIX.out and PREFIX.err.
recall() { # prefix description
    LAST=$2
    CODE=$(cat "$1.code")
    OUT=$(cat "$1.out")
    ERR=$(cat "$1.err")
}

# A jq expression over the last report.
field() {
    printf '%s' "$OUT" | jq -r "$1"
}

# ── Assertions ───────────────────────────────────────────────────────────

ok() {
    CHECKS=$((CHECKS + 1))
    printf '  ok    %s\n' "$1"
}

fail() {
    {
        printf '  FAIL  %s\n' "$1"
        printf '        command  %s\n' "${LAST#"$ML" }"
        printf '        exit     %s\n' "$CODE"
        [ -z "$OUT" ] || printf '        report   %s\n' "$(printf '%s' "$OUT" | jq -c . 2>/dev/null || printf '%s' "$OUT")"
        [ -z "$ERR" ] || printf '        stderr   %s\n' "$ERR"
        printf '        workdir  %s\n' "$WORK"
    } >&2
    exit 1
}

expect_exit() { # N description
    [ "$CODE" -eq "$1" ] || fail "$2: expected exit $1, got $CODE"
}

# Each KEY=VALUE names a field of the last report and the value it must have.
expect_fields() { # description KEY=VALUE...
    local desc=$1 kv have
    shift
    for kv in "$@"; do
        have=$(field ".${kv%%=*}")
        [ "$have" = "${kv#*=}" ] || fail "$desc: ${kv%%=*} is \`$have\`, expected \`${kv#*=}\`"
    done
}

# The step was allowed: exit 0, and these fields say what was decided.
expect_allowed() { # description KEY=VALUE...
    local desc=$1
    shift
    expect_exit 0 "$desc"
    expect_fields "$desc" "$@"
    ok "$desc"
}

# The ledger refused the step: exit 2, with this code. `recorded` is true
# when the refusal is in the chain, false when the type system said no
# before the engine could be asked.
expect_refused() { # CODE recorded description
    expect_exit 2 "$3"
    expect_fields "$3" "refused=$1" "recorded=$2"
    if [ "$2" = true ]; then
        ok "$3: $1, recorded"
    else
        ok "$3: $1, nothing to record"
    fi
}

# The command could not decide: exit 1, this on stderr, no decision.
expect_undecided() { # stderr-substring description
    expect_exit 1 "$2"
    case "$ERR" in
    *"$1"*) ok "$2" ;;
    *) fail "$2: stderr does not mention \`$1\`" ;;
    esac
}

# The last report's `detail` mentions this text.
expect_detail() { # substring description
    case "$(field .detail)" in
    *"$1"*) ;;
    *) fail "$2: detail is \`$(field .detail)\`, expected it to mention \`$1\`" ;;
    esac
}

# The chain of CTX is exactly this sequence of `event` or `denied:CODE`.
expect_chain() { # ctx "event event:CODE ..."
    local have
    step log --context "$1"
    expect_exit 0 "reading the chain of $1"
    have=$(field '[.events[] | .event + (if .code then ":" + .code else "" end)] | join(" ")')
    [ "$have" = "$2" ] || fail "chain of $1 is [$have], expected [$2]"
    ok "chain of ${1:0:12}…  $2"
}

# The run's mandate holds exactly N contexts in this state.
expect_contexts() { # state N description
    local have
    step contexts --mandate "$MANDATE_ID" --state "$1" --limit 1000
    expect_exit 0 "$3"
    have=$(field '.contexts | length')
    [ "$have" -eq "$2" ] || fail "$3: $have $1 contexts, expected $2"
    ok "$3"
}

# Export CTX's chain signed by this host, then verify it from the file
# alone, with no database in the environment. Then alter one event and
# check the altered copy is refused: a verifier that passes everything
# proves nothing.
verify_evidence() { # ctx
    local bundle="$WORK/evidence-$1.json" events
    step evidence "$1" --out "$bundle" --sign "$HOST_KEY"
    expect_exit 0 "exporting the evidence of $1"
    events=$(field .events)

    capture env -u ML_DATABASE_URL "$ML" --json verify "$bundle" --signer "$HOST_KEY.pub"
    expect_exit 0 "verifying $bundle"
    expect_fields "verifying $bundle" verified=true "context=$1" "events=$events" signed=true
    ok "evidence of ${1:0:12}… verifies without the database ($events events, signed)"

    jq '.bundle.events[0].at += 1' "$bundle" >"$bundle.altered"
    capture env -u ML_DATABASE_URL "$ML" --json verify "$bundle.altered" --signer "$HOST_KEY.pub"
    expect_exit 2 "verifying an altered copy"
    expect_fields "verifying an altered copy" verified=false refused=HASH_MISMATCH
    ok "an altered copy is refused: HASH_MISMATCH"
}

# ── Fixtures ─────────────────────────────────────────────────────────────

# A payment reference unique to this run: nonces are spent forever per
# rail, so a fixed one would replay against an earlier run. The rail's
# suffixes (-ok, -fail, -reorg) stay at the end.
ref() { # tag
    printf '%s-%s' "$RUN" "$1"
}

# Sign a mandate for this run's principal into WORK/NAME.json and print
# the path: grocery at bigbasket.com and *.zepto.com, 2,000 INR per
# purchase, 8,000 in all, five a day, merchant-signed carts only, valid
# from 2023 to 2100 so the wall clock is inside it. FILTER is a jq
# expression over the body, for a scope shaped differently.
sign_mandate() { # name [filter]
    jq -n --arg id "$MANDATE_ID" --arg principal "$PRINCIPAL" '{
        id: $id,
        principal: $principal,
        agent: "agent:shopper",
        scope: {
            merchants: ["bigbasket.com", "*.zepto.com"],
            categories: ["grocery"],
            currency: "INR",
            max_per_txn: { amount: "2000.00", currency: "INR" },
            max_total: { amount: "8000.00", currency: "INR" },
            valid_from: 1700000000,
            valid_until: 4102444800,
            velocity: { max_count: 5, window_secs: 86400 },
            min_attestation: "merchant_signed"
        },
        issued_at: 1700000000
    }' | jq "${2:-.}" >"$WORK/$1.body.json"
    "$ML" mandate sign "$WORK/$1.body.json" --key "$USER_KEY" --out "$WORK/$1.json" >/dev/null
    printf '%s' "$WORK/$1.json"
}

# A grocery cart at bigbasket.com for TOTAL INR, as the agent built it, in
# WORK/NAME.raw.json. FILTER is a jq expression over the cart.
raw_cart() { # name total [filter]
    jq -n --arg total "$2" '{
        merchant: "bigbasket.com",
        total: { amount: $total, currency: "INR" },
        category: "grocery",
        items: [{ sku: "milk-1l", qty: 2 }]
    }' | jq "${3:-.}" >"$WORK/$1.raw.json"
    printf '%s' "$WORK/$1.raw.json"
}

# The same cart signed by the merchant under key id `bb`, in
# WORK/NAME.json; its hash in WORK/NAME.hash.
cart() { # name total [filter]
    raw_cart "$@" >/dev/null
    "$ML" --json cart sign "$WORK/$1.raw.json" --key "$MERCHANT_KEY" --key-id bb --out "$WORK/$1.json" |
        jq -r .hash >"$WORK/$1.hash"
    printf '%s' "$WORK/$1.json"
}

cart_hash() { # name
    cat "$WORK/$1.hash"
}

# `ml authorize` with this run's trust configuration.
authorize() { # mandate-file cart-file request-key
    step authorize --mandate "$1" --cart "$2" --request-key "$3" "${TRUST[@]}"
}

# ── Lifecycle of a scenario ──────────────────────────────────────────────

CLOCK_USED=0

# The scenario freezes the clock. It is global to the database, so it goes
# back to wall time when the scenario ends, however it ends.
use_clock() {
    CLOCK_USED=1
}

scenario_exit() {
    local rc=$?
    if [ "$CLOCK_USED" -eq 1 ]; then
        "$ML" clock reset >/dev/null 2>&1 || printf '  warning: could not reset the clock\n' >&2
    fi
    if [ "$rc" -eq 0 ]; then
        printf '  PASS  %s (%d checks)\n' "$SCENARIO" "$CHECKS"
        [ -n "${KEEP:-}" ] || rm -rf "$WORK"
    else
        printf '  FAIL  %s\n' "$SCENARIO" >&2
    fi
    exit "$rc"
}

# A fresh key pair in WORK/NAME.key; the path is printed.
new_key() { # name
    "$ML" keys new --out "$WORK/$1.key" >/dev/null
    printf '%s' "$WORK/$1.key"
}

# The body behind WORK/NAME.json signed again by a key of an attacker's
# own, into WORK/NAME.forged.json; the path is printed.
forge_mandate() { # name
    "$ML" mandate sign "$WORK/$1.body.json" --key "$(new_key "$1.attacker")" --out "$WORK/$1.forged.json" >/dev/null
    printf '%s' "$WORK/$1.forged.json"
}

# Begin a scenario: a working directory, a run suffix, three key pairs —
# the principal, the merchant, this host — and the trust flags that make
# the engine accept the first two.
scenario() { # name
    local tmp=${TMPDIR:-/tmp}
    SCENARIO=$1
    WORK=$(mktemp -d "${tmp%/}/ml-scenario-$1.XXXXXX")
    RUN=$(printf '%s-%x-%04x%04x' "$1" "$$" "$RANDOM" "$RANDOM")
    PRINCIPAL="user:$RUN"
    MANDATE_ID="mnd-$RUN"
    USER_KEY="$WORK/user.key"
    MERCHANT_KEY="$WORK/merchant.key"
    HOST_KEY="$WORK/host.key"
    trap scenario_exit EXIT
    printf '%s\n' "$SCENARIO"
    new_key user >/dev/null
    new_key merchant >/dev/null
    new_key host >/dev/null
    TRUST=(--trust "$PRINCIPAL=$USER_KEY.pub" --merchant-key "bb=$MERCHANT_KEY.pub")
}
