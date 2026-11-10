"""Check that incomplete measurements or a bad run cannot pass the gate."""
import unittest
import copy
import json
from pathlib import Path

from real_model_gate import apply_rename, evaluate, has_symbol, offset_at, position, queue_samples, symbol_ranges, valid_references, validate_policy


class RealModelGateTests(unittest.TestCase):
    def test_policy_cannot_silently_drop_a_model_metric_or_stress_phase(self):
        policy = json.loads((Path(__file__).resolve().parent.parent / "benchmarks/real-model-performance.json").read_text())
        validate_policy(policy)
        changes = [lambda p: p["models"].pop("arm"),
                   lambda p: p["limitsMs"].pop("hover"),
                   lambda p: p.update(reloadEvery=p["cycles"] + 1),
                   lambda p: p.update(minimumCancelledResponses=0),
                   lambda p: p.pop("renameEditFormat"),
                   lambda p: p["models"]["arm"]["largeDocuments"].pop(),
                   lambda p: p["models"]["riscv"]["largeDocuments"][0].update(minimumBytes=1),
                   lambda p: p["limitsMs"]["hover"].update(p95=float("inf"))]
        for change in changes:
            broken = copy.deepcopy(policy)
            change(broken)
            with self.assertRaises(ValueError):
                validate_policy(broken)

    def test_every_run_must_meet_absolute_limits(self):
        policy = {"limitsMs": {"completion": {"p95": 100, "p99": 200}}}
        checks = evaluate([{"completion": [20, 50]}, {"completion": [20, 300]}], policy)
        self.assertFalse(all(check["passed"] for check in checks))
        self.assertEqual(checks[-1]["observedMs"], 300)

    def test_missing_metric_is_failure(self):
        checks = evaluate([{}], {"limitsMs": {"interactive_queue": {"p95": 100}}})
        self.assertFalse(checks[0]["passed"])
        self.assertIsNone(checks[0]["observedMs"])

    def test_worker_queue_is_distinct_from_execution_and_background(self):
        text = "\n".join([
            "sail_lsp::latency request_worker method=textDocument/completion queue_us=1200 handler_us=9000",
            "sail_lsp::latency request_worker method=textDocument/references queue_us=90000 handler_us=100",
            "sail_lsp::latency request_complete method=textDocument/hover queue_us=500",
            "unrelated request_worker method=textDocument/definition queue_us=9000",
        ])
        self.assertEqual(queue_samples(text), [1.2])

    def test_completion_requires_expected_symbol(self):
        self.assertFalse(has_symbol({"error": {"code": -32801}}, "target"))
        self.assertFalse(has_symbol({"result": {"items": [{"label": "other"}]}}, "target"))
        self.assertTrue(has_symbol({"result": {"items": [{"label": "target"}]}}, "target"))

    def test_large_references_require_exact_current_name_ranges(self):
        source = "// 🐟\nfunction probe(x) = x\nlet x = probe(1)\n"
        path = Path("/tmp/large.sail")
        locations = [{"uri": path.as_uri(), "range": span} for span in symbol_ranges(source, "probe")]
        self.assertTrue(valid_references({"result": locations}, path, source, "probe"))
        self.assertFalse(valid_references({"result": locations[:1]}, path, source, "probe"))
        broken = copy.deepcopy(locations)
        broken[0]["range"]["start"]["character"] += 1
        self.assertFalse(valid_references({"result": broken}, path, source, "probe"))

    def test_applied_rename_checks_versions_and_both_occurrences(self):
        source = "// 🐟\nfunction probe(x) = x\nlet x = probe(1)\n"
        path = Path("/tmp/large.sail")
        edits = [{"range": span, "newText": "renamed"} for span in symbol_ranges(source, "probe")]
        response = {"result": {"documentChanges": [{"textDocument": {"uri": path.as_uri(), "version": 7}, "edits": edits}]}}
        self.assertEqual(apply_rename(response, path, source, "probe", "renamed", 7), source.replace("probe", "renamed"))
        for change in [lambda r: r["result"]["documentChanges"][0]["textDocument"].update(version=6),
                       lambda r: r["result"]["documentChanges"][0]["textDocument"].update(version=None),
                       lambda r: r["result"]["documentChanges"][0]["edits"].pop(),
                       lambda r: r["result"]["documentChanges"][0]["textDocument"].update(uri="file:///tmp/other.sail")]:
            invalid = copy.deepcopy(response)
            change(invalid)
            with self.assertRaises(AssertionError):
                apply_rename(invalid, path, source, "probe", "renamed", 7)

    def test_utf16_ranges_roundtrip_after_unicode_text(self):
        source = "// 🐟 text\nfunction x() = 1\n"
        for offset in range(len(source) + 1):
            self.assertEqual(offset_at(source, position(source, offset)), offset)

    def test_large_body_edits_preserve_original_model_contents(self):
        from real_model_gate import large_interactions

        class FirstChange(Exception):
            pass

        class Client:
            def send(self, method, params):
                self.method = method
                self.params = params
                raise FirstChange()

        original = "function existing(argument : int) -> int = argument\n"
        for cycle in [0, 1]:
            client = Client()
            fixture = {"path": Path("/tmp/large.sail"), "original": original,
                       "symbol": "__sail_perf_large_0", "version": 1}
            with self.assertRaises(FirstChange):
                large_interactions(client, fixture, cycle, {})
            self.assertEqual(client.method, "textDocument/didChange")
            changed = client.params["contentChanges"][0]["text"]
            self.assertTrue(changed.startswith(original), changed)
            self.assertIn(f"function __sail_perf_large_0(argument : int) -> int = argument + {cycle}\n", changed)


if __name__ == "__main__":
    unittest.main()
