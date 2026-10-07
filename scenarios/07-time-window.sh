#!/usr/bin/env bash
# Threat model A8 — expired mandate (P17), and the validity window.
#
# The sandbox clock is what every engine process reads as "now", so a
# mandate's window and its velocity limit can be walked through without
# waiting. Before `valid_from` the mandate is not yet valid; after
# `valid_until` it is expired; inside the window, the velocity limit
# refuses the purchase that would exceed it — until the window has passed.
# Every event carries the frozen instant it was decided at.

# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"
scenario time-window
use_clock

opens=1800000000              # 2027-01-15
closes=$((opens + 7 * 86400)) # a week later
window=3600                   # three purchases an hour
mandate=$(sign_mandate mandate ".scope.valid_from = $opens
    | .scope.valid_until = $closes
    | .issued_at = $opens
    | .scope.velocity = { max_count: 3, window_secs: $window }")

step clock set $((opens - 1))
expect_allowed "the clock is frozen a second before the mandate is valid" source=frozen now=$((opens - 1))
authorize "$mandate" "$(cart early "10.00")" "order-early"
expect_refused MANDATE_NOT_YET_VALID true "a purchase before the window opens"
early=$(field .context)

step clock set $((closes + 1))
expect_allowed "the clock is frozen a second after the mandate lapses" now=$((closes + 1))
authorize "$mandate" "$(cart late "10.00")" "order-late"
expect_refused MANDATE_EXPIRED true "a purchase after the window closes"
late=$(field .context)

step clock set "$opens"
expect_allowed "the clock is frozen at the instant the window opens" now="$opens"
for i in 1 2 3; do
    authorize "$mandate" "$(cart "c$i" "10.00")" "order-$i"
    expect_allowed "purchase $i of 3 inside the hour" state=authorized
done
third=$(field .context)
authorize "$mandate" "$(cart c4 "10.00")" "order-4"
expect_refused VELOCITY_EXCEEDED true "a fourth purchase inside the hour"
expect_detail "3 authorizations already" "the refusal counts the window"
fourth=$(field .context)

step clock advance $((window + 1))
expect_allowed "an hour and a second pass" now=$((opens + window + 1))
authorize "$mandate" "$(cart c5 "10.00")" "order-5"
expect_allowed "the next purchase, in a new window" state=authorized
fifth=$(field .context)

step log --context "$fifth"
expect_allowed "the event carries the instant the clock was frozen at" "events[0].at=$((opens + window + 1))"

expect_chain "$early" "denied:MANDATE_NOT_YET_VALID"
expect_chain "$late" "denied:MANDATE_EXPIRED"
expect_chain "$fourth" "denied:VELOCITY_EXCEEDED"
expect_contexts authorized 4 "four purchases authorized, three refused"
verify_evidence "$third"
verify_evidence "$fourth"
