#!/usr/bin/env python3
"""Tests for the residual benchmark — KT-197 DoD 6.

This script is a GATE, so the cases that matter are the ones where it would pass
for the wrong reason: a format change parsed as clean, a missing filter counted as
present, a rare command ranked above a heavily-used one.
"""

import unittest

from rtk_residual_benchmark import (
    COMPRESSION_FLOORS,
    floor_violations,
    parse_gain,
    weakest_filters,
)

# Verbatim from `rtk gain` on this fleet.
GAIN = """RTK Token Savings (Global Scope)

Total commands:    47035
Tokens saved:      74.7M (65.4%)

By Command
───────────────────────────────────────────────────────────────────────
  #  Command                   Count  Saved    Avg%    Time  Impact
───────────────────────────────────────────────────────────────────────
 1.  rtk vitest run              165  16.0M   96.6%   17.7s  ██████████
 2.  rtk cargo test --lib        105   7.2M   96.4%   47.9s  █████░░░░░
 4.  rtk lint eslint src/         13   6.4M   99.9%   15.1s  ████░░░░░░
 5.  rtk find                    261   5.6M   60.6%   791ms  ███░░░░░░░
 6.  rtk read                    985   4.6M   10.6%     2ms  ███░░░░░░░
 9.  rtk cargo test --lib ...      36   2.5M   94.7%   1m49s  ██░░░░░░░░
───────────────────────────────────────────────────────────────────────
"""


class ParsingTheGainTable(unittest.TestCase):
    def test_reads_every_per_command_row(self):
        rows = parse_gain(GAIN)
        self.assertEqual(len(rows), 6)
        self.assertEqual(rows[0]["command"], "rtk vitest run")
        self.assertEqual(rows[0]["count"], 165)
        self.assertEqual(rows[0]["percent"], 96.6)

    def test_ignores_the_header_and_the_totals(self):
        # "Total commands: 47035" is not a per-command row. Parsing it as one would
        # invent a filter at 0% and rank it first.
        commands = [row["command"] for row in parse_gain(GAIN)]
        self.assertNotIn("Total commands:", commands)
        self.assertTrue(all(cmd.startswith("rtk") for cmd in commands), commands)

    def test_only_a_numbered_row_is_a_command_row(self):
        # The rank prefix is what marks a line as a per-command row. The test above
        # passes for a different reason — the header has no numeric columns — so it
        # would keep passing if the anchor were dropped. This is the one that fails.
        self.assertEqual(
            parse_gain("     rtk read                  985   4.6M   10.6%   2ms\n"),
            [],
            "a line with no rank prefix was read as a command row",
        )
        self.assertEqual(
            len(parse_gain(" 6.  rtk read                  985   4.6M   10.6%   2ms\n")),
            1,
        )

    def test_a_thousands_separator_survives(self):
        rows = parse_gain(" 1.  rtk read                  1,985   4.6M   10.6%   2ms\n")
        self.assertEqual(rows[0]["count"], 1985)

    def test_an_unparsable_table_yields_nothing_rather_than_a_guess(self):
        self.assertEqual(parse_gain("some completely different output"), [])


class TheFloors(unittest.TestCase):
    def test_the_measured_fleet_passes(self):
        # The gate has to be passable on real data, or it is just a permanent
        # refusal that gets disabled.
        self.assertEqual(floor_violations(parse_gain(GAIN)), [])

    def test_a_regression_below_a_floor_fails(self):
        degraded = GAIN.replace("96.6%", "40.0%")
        violations = floor_violations(parse_gain(degraded))
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0]["filter"], "vitest")
        self.assertEqual(violations[0]["measured"], 40.0)

    def test_every_variant_of_a_filter_is_checked_not_just_the_first(self):
        # `cargo test --lib` appears twice with different rates. Checking only the
        # first would let the worse variant regress unseen.
        degraded = GAIN.replace("94.7%", "10.0%")
        violations = floor_violations(parse_gain(degraded))
        self.assertTrue(
            any(v["measured"] == 10.0 for v in violations),
            f"the second variant was not checked: {violations}",
        )

    def test_a_filter_that_vanished_is_a_violation_not_a_pass(self):
        # THE case a gate fails at. A floor with nothing to check proves nothing,
        # and skipping it silently is how a gate stops gating.
        without_lint = "\n".join(
            line for line in GAIN.splitlines() if "lint" not in line
        )
        violations = floor_violations(parse_gain(without_lint))
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0]["filter"], "lint")
        self.assertIsNone(violations[0]["measured"])
        self.assertIn("no row", violations[0]["why"])

    def test_the_floors_are_documented_as_measured_values(self):
        # Guarding the mistake this file was written after: floors set to round
        # numbers before measuring. Real measurements are rarely round.
        self.assertEqual(COMPRESSION_FLOORS["cargo test"], 94.5)
        self.assertLess(
            COMPRESSION_FLOORS["cargo test"],
            96.0,
            "a floor above the lowest measured variant fails on a healthy fleet",
        )


class RankingTheResidual(unittest.TestCase):
    def test_a_heavily_used_weak_filter_outranks_a_rare_perfect_one(self):
        # 985 calls at 10.6% cost far more than two calls at 0%. Ranking by
        # percentage alone would send the work to the wrong filter.
        weakest = weakest_filters(parse_gain(GAIN))
        self.assertEqual(weakest[0]["command"], "rtk read")
        self.assertEqual(weakest[1]["command"], "rtk find")

    def test_a_rare_zero_percent_command_does_not_take_first_place(self):
        with_rare = GAIN + " 7.  rtk something                 2   0     0.0%   1ms\n"
        weakest = weakest_filters(parse_gain(with_rare))
        self.assertEqual(weakest[0]["command"], "rtk read")

    def test_a_perfect_filter_scores_zero(self):
        rows = parse_gain(" 1.  rtk perfect                100   9M   100.0%   1s\n")
        self.assertEqual(weakest_filters(rows)[0]["residual_score"], 0)


if __name__ == "__main__":
    unittest.main()
