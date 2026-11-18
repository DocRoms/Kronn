#!/usr/bin/env python3
"""Tests of rustc_memory.py: what a peak is, and when the gate fails."""
import importlib.util
import pathlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "rustc_memory", pathlib.Path(__file__).parent / "rustc_memory.py")
rustc_memory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rustc_memory)

GB = 1024 * 1024


class RustcMemory(unittest.TestCase):
    def run_gate(self, text, *extra):
        with tempfile.TemporaryDirectory() as tmp:
            log = pathlib.Path(tmp) / "log"
            if text is not None:
                log.write_text(text)
            return rustc_memory.main([str(log), *extra])

    def test_the_highest_peak_of_a_crate_is_kept_and_the_test_build_is_apart(self):
        found = rustc_memory.peaks(f"{2 * GB} kronn_l0\n{3 * GB} kronn_l0\n{5 * GB} kronn_l0 test\n")
        self.assertEqual(found, {"kronn_l0": 3 * GB, "kronn_l0 test": 5 * GB})

    def test_lines_that_are_no_measure_are_ignored(self):
        text = f"Command exited with non-zero status 1\n{GB} kronn\n\n12 not a crate name!\n"
        self.assertEqual(rustc_memory.peaks(text), {"kronn": GB})

    def test_a_crate_under_the_cap_passes(self):
        self.assertEqual(self.run_gate(f"{7 * GB} kronn_l1 test\n"), 0)

    def test_a_crate_over_the_cap_fails(self):
        self.assertEqual(self.run_gate(f"{GB} kronn_l0\n{9 * GB} kronn_l1 test\n"), 1)
        self.assertEqual(rustc_memory.over({"a": 9 * GB, "b": GB}, 8.0), ["a"])

    def test_the_cap_is_the_last_passing_value(self):
        self.assertEqual(self.run_gate(f"{8 * GB} kronn\n"), 0)
        self.assertEqual(self.run_gate(f"{8 * GB + 1} kronn\n"), 1)

    def test_the_cap_can_be_set_on_the_command_line(self):
        self.assertEqual(self.run_gate(f"{3 * GB} kronn\n", "--max-gb", "2"), 1)
        self.assertEqual(self.run_gate(f"{3 * GB} kronn\n", "--max-gb", "4"), 0)

    def test_a_cache_hit_measures_nothing_and_passes(self):
        self.assertEqual(self.run_gate(None), 0)
        self.assertEqual(self.run_gate(""), 0)

    def test_no_log_argument_is_a_usage_error(self):
        self.assertEqual(rustc_memory.main([]), 2)


if __name__ == "__main__":
    unittest.main()
