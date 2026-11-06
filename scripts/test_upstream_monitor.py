#!/usr/bin/env python3
"""Protocol/error classification tests; real compiler comparisons run separately."""
from pathlib import Path
import sys
import tempfile
import unittest

from upstream_monitor import (
    InfrastructureError, Monitor, UnsupportedConfiguration, parse_graph, variable_args,
)


class MonitorTests(unittest.TestCase):
    def test_graph_reverses_requires_and_keeps_isolated_nodes(self):
        graph, nodes = parse_graph('digraph DEPS {\n "corelib" [];\n "A" [];\n "B" [];\n "C" [];\n "A" -> "B" [style=solid];\n "corelib" -> "A" [];\n}')
        self.assertEqual(graph, {"B": ["A"]})
        self.assertEqual(nodes, ["A", "B", "C"])

    def test_invalid_graph_is_infrastructure_not_an_empty_graph(self):
        for output in ("", "Error: failed", "digraph DEPS {", 'digraph D {\n A -> B;\n}'):
            with self.subTest(output=output), self.assertRaises(InfrastructureError):
                parse_graph(output)

    def test_boolean_strings_cannot_be_reported_as_matching_booleans(self):
        self.assertEqual(variable_args({"RMEM": True, "ARCH": "A64"}), ["--variable", "ARCH=A64", "--variable", "RMEM=true"])
        for value in ("true", "false", " A64"):
            with self.subTest(value=value), self.assertRaises(UnsupportedConfiguration):
                variable_args({"X": value})

    @unittest.skipIf(sys.platform == "win32", "executable POSIX fixture")
    def test_graph_process_failure_is_never_equal_to_empty_requires(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            manifest = directory / "empty.sail_project"
            manifest.write_text("A {}\n")
            compiler = directory / "compiler"
            compiler.write_text('#!/bin/sh\ncase "$*" in *--ddump-project-depgraph*) echo failed >&2; exit 3;; *) printf "\\n";; esac\n')
            compiler.chmod(0o700)
            oracle = directory / "oracle"
            oracle.write_text('#!/bin/sh\ncat >/dev/null\nprintf \'%s\' \'{"accepted":true,"modules":[],"files":[],"requires":{}}\'\n')
            oracle.chmod(0o700)
            report = {"cases": []}
            Monitor(compiler, oracle, 2, report).compare("negative", manifest, {})
            kinds = [check["classification"] for check in report["cases"][0]["checks"]]
            self.assertEqual(kinds, ["agreement", "infrastructure_failure"])
            # Keep actual nonzero status/stderr in the report for triage.
            self.assertIn('"exit_code": 3', report["cases"][0]["checks"][-1]["error"])

    @unittest.skipIf(sys.platform == "win32", "executable POSIX fixture")
    def test_timed_out_oracle_fails_instead_of_reducing_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            manifest = directory / "empty.sail_project"
            manifest.write_text("A {}\n")
            oracle = directory / "oracle"
            oracle.write_text('#!/bin/sh\nexec sleep 5\n')
            oracle.chmod(0o700)
            report = {"cases": []}
            Monitor(oracle, oracle, 0.05, report).compare("timeout", manifest, {})
            check = report["cases"][0]["checks"][0]
            self.assertEqual(check["classification"], "infrastructure_failure")
            self.assertIn("timed out", check["error"])

    @unittest.skipIf(sys.platform == "win32", "executable POSIX fixture")
    def test_missing_model_source_is_infrastructure_not_a_semantic_difference(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            manifest = directory / "missing.sail_project"
            manifest.write_text("A { files missing.sail }\n")
            oracle = directory / "oracle"
            oracle.write_text('#!/bin/sh\ncat >/dev/null\nprintf \'%s\' \'{"accepted":true,"modules":["A"],"files":["missing.sail"],"requires":{}}\'\n')
            oracle.chmod(0o700)
            report = {"cases": []}
            Monitor(directory / "compiler-must-not-run", oracle, 2, report).compare("missing", manifest, {})
            checks = report["cases"][0]["checks"]
            self.assertEqual(len(checks), 1)
            self.assertEqual(checks[0]["classification"], "infrastructure_failure")
            self.assertIn("source input does not exist", checks[0]["error"])


if __name__ == "__main__":
    unittest.main()
