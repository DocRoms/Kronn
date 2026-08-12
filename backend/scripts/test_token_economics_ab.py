"""Tests for the controlled KT-192/KT-198 benchmark."""

import unittest

import token_economics_ab as bench


class PromptVariants(unittest.TestCase):
    def test_every_variant_carries_the_same_canonical_evidence(self):
        for variant in bench.VARIANTS:
            prompt = bench.build_prompt(variant)
            for value in bench.EXPECTED.values():
                self.assertIn(value, prompt, (variant, value))

    def test_context_is_reduced_in_the_intended_order(self):
        sizes = {variant: len(bench.build_prompt(variant).encode()) for variant in bench.VARIANTS}
        self.assertGreaterEqual(sizes["B"], bench.LONG_SESSION_CONTEXT_BYTES)
        self.assertGreater(sizes["A"], sizes["B"])
        self.assertGreater(sizes["B"], sizes["D"])
        self.assertGreater(sizes["D"], sizes["C"])

    def test_unknown_variant_fails_closed(self):
        with self.assertRaisesRegex(ValueError, "unknown variant"):
            bench.build_prompt("E")


class Scoring(unittest.TestCase):
    def test_exact_answer_scores_four(self):
        self.assertEqual(bench.score(dict(bench.EXPECTED)), 4)

    def test_plausible_but_wrong_value_loses_a_point(self):
        answer = dict(bench.EXPECTED)
        answer["cause"] = "polling_is_slow"
        self.assertEqual(bench.score(answer), 3)


class Aggregation(unittest.TestCase):
    @staticmethod
    def sample(variant, raw, duration, quality=4):
        return bench.Run(
            provider="codex", variant=variant, duration_ms=duration,
            raw_traffic_tokens=raw, non_cached_input_tokens=raw,
            cache_read_tokens=0, cache_write_tokens=None, output_tokens=0,
            quality_score=quality, success=quality == 4,
        )

    def test_nearest_rank_p90_is_not_the_mean(self):
        self.assertEqual(bench.percentile([1, 2, 3, 100], 0.9), 100)

    def test_summary_compares_a_to_d_without_hiding_quality(self):
        runs = [
            self.sample("A", 100, 1000), self.sample("A", 120, 1200),
            self.sample("D", 40, 900), self.sample("D", 50, 1000),
        ]
        summary = bench.summarise(runs)["codex"]
        self.assertEqual(summary["A"]["raw_traffic_tokens_median"], 110)
        self.assertEqual(summary["D"]["raw_traffic_tokens_p90"], 50)
        self.assertAlmostEqual(summary["comparison"]["median_raw_traffic_reduction_pct"], 59.09)
        self.assertTrue(summary["comparison"]["quality_not_degraded"])


if __name__ == "__main__":
    unittest.main()
