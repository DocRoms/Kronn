#!/usr/bin/env python3
"""Tests for the re-review benchmark — KT-196 DoD 7.

A benchmark that reports the wrong number is worse than none: it gets quoted. The
first version of this script measured published message bytes and found a NEGATIVE
saving, which was arithmetically right and about the wrong quantity. So what is
tested here is the accounting itself — what a pass is charged for, and what it is
not.
"""

import unittest

from rereview_benchmark import FILE_PATH, SITE, measure


def agent(content, author="ClaudeCode"):
    return ("Agent", author, content)


def user(content):
    return ("User", "-", content)


class DetectingAReviewPass(unittest.TestCase):
    def test_a_message_naming_a_source_file_is_a_pass(self):
        result = measure([agent("look at backend/src/api/disc.rs please")])
        self.assertEqual(result["passes"], 1)

    def test_a_message_with_no_file_is_not_a_pass(self):
        # Chatter is not a review pass, and counting it would inflate every
        # per-pass figure downward.
        result = measure([agent("looks good to me, shipping")])
        self.assertEqual(result["passes"], 0)

    def test_a_user_message_is_never_a_pass(self):
        # The saving is about what an AGENT is sent. A human naming a file is not
        # a review pass being paid for.
        result = measure([user("check backend/src/api/disc.rs")])
        self.assertEqual(result["passes"], 0)

    def test_a_bare_word_is_not_mistaken_for_a_file(self):
        for text in ["the ratio was 1.5x", "see section 3.3", "version 0.9.4"]:
            self.assertIsNone(FILE_PATH.search(text), text)

    def test_a_site_carries_its_line_and_is_bucketed_by_ten(self):
        # Same bucketing as the ledger's fingerprint, so the two agree on what one
        # cause is.
        self.assertEqual(SITE.findall("src/a.rs:12 and src/a.rs:17"), [("src/a.rs", "12"), ("src/a.rs", "17")])
        result = measure([agent("src/a.rs:12"), agent("src/a.rs:17")])
        self.assertEqual(result["sites"], 1, "12 and 17 are the same 10-line bucket")
        self.assertEqual(result["repeated_sites"], 1)

    def test_two_distant_lines_are_two_sites(self):
        result = measure([agent("src/a.rs:12"), agent("src/a.rs:212")])
        self.assertEqual(result["sites"], 2)
        self.assertEqual(result["repeated_sites"], 0)


class WhatAPassIsChargedFor(unittest.TestCase):
    def test_a_pass_is_not_charged_for_its_own_message(self):
        # An agent is not sent the message it has not written yet. Charging for it
        # would inflate the before-figure by the size of every report.
        result = measure([agent("src/a.rs:1 " + "x" * 1000)])
        self.assertEqual(result["cold"], 0)
        self.assertEqual(result["warm"], 0)

    def test_a_cold_pass_is_charged_for_everything_before_it(self):
        result = measure([user("a" * 100), agent("src/a.rs:1")])
        self.assertEqual(result["cold"], 100)

    def test_the_first_pass_costs_the_same_cold_or_warm(self):
        # Nothing has been seen yet, so the two accountings must agree — a
        # divergence here would mean one of them double-counts.
        result = measure([user("a" * 500), agent("src/a.rs:1")])
        self.assertEqual(result["cold"], result["warm"])

    def test_a_warm_pass_pays_only_for_what_arrived_since_its_last_one(self):
        rows = [
            user("a" * 100),
            agent("src/a.rs:1"),          # cold 100, warm 100
            user("b" * 50),
            agent("src/b.rs:1"),          # cold 100+10+50, warm 10+50
        ]
        result = measure(rows)
        first = len("src/a.rs:1".encode())
        self.assertEqual(result["cold"], 100 + (100 + first + 50))
        self.assertEqual(result["warm"], 100 + (first + 50))

    def test_two_authors_do_not_share_a_warm_cursor(self):
        # Each session has its own context. Sharing the cursor would credit one
        # agent for what another had already read.
        rows = [
            user("a" * 100),
            agent("src/a.rs:1", author="ClaudeCode"),
            agent("src/b.rs:1", author="Codex"),
        ]
        result = measure(rows)
        # Codex has never passed before, so it is charged for everything so far.
        self.assertEqual(result["warm"], 100 + (100 + len("src/a.rs:1".encode())))

    def test_bytes_are_counted_in_utf8_not_characters(self):
        # A byte count is what a transport and a tokeniser both see; a character
        # count would understate every accented French message in these threads.
        result = measure([user("é" * 10), agent("src/a.rs:1")])
        self.assertEqual(result["cold"], 20)


class RepetitionAcrossPasses(unittest.TestCase):
    def test_a_file_named_in_two_passes_counts_as_repeated(self):
        result = measure([agent("src/a.rs here"), agent("src/a.rs again")])
        self.assertEqual(result["files"], 1)
        self.assertEqual(result["repeated"], 1)
        self.assertEqual(result["mentions"], 2)

    def test_a_file_named_twice_in_one_pass_is_one_mention(self):
        # The metric is how many separate passes re-covered a file. Counting
        # mentions inside one message would report repetition where there is none.
        result = measure([agent("src/a.rs and src/a.rs")])
        self.assertEqual(result["mentions"], 1)
        self.assertEqual(result["repeated"], 0)

    def test_an_empty_discussion_measures_to_zero_not_an_error(self):
        result = measure([])
        self.assertEqual(result["passes"], 0)
        self.assertEqual(result["cold"], 0)
        self.assertEqual(result["files"], 0)


if __name__ == "__main__":
    unittest.main()
