#!/usr/bin/env python3
"""Summarize SAIL_LOG=sail_lsp::latency=debug logs from editor_probe.py.

All server durations use monotonic clocks. Worker samples include retries;
completion samples measure from main-loop receipt, excluding transport and
messages still waiting in the connection channel. Compare with probe latency.
"""
import argparse
import json
import math
import re
from collections import defaultdict
from pathlib import Path


def summarize(text):
    samples = defaultdict(list)
    for line in text.splitlines():
        if 'sail_lsp::latency' not in line:
            continue
        # Strip terminal styling before matching fields.
        line = re.sub(r'\x1b\[[0-9;]*m', '', line)
        event = re.search(r'\b(request_worker|request_complete|process_changes|task_drain|apply_change)\b', line)
        if event is None:
            continue
        method = re.search(r'\bmethod=(\S+)', line)
        group = event[0] + (':' + method[1] if method else '')
        for field, value in re.findall(r'\b(\w+_us)=(\d+)', line):
            samples[group + ':' + field].append(int(value) / 1000)
    result = {}
    for key, values in sorted(samples.items()):
        values.sort()
        result[key] = {
            'count': len(values),
            'p50_ms': values[math.ceil(len(values) * .5) - 1],
            'p95_ms': values[math.ceil(len(values) * .95) - 1],
            'max_ms': values[-1],
        }
    if not result:
        raise ValueError('No latency events found; enable SAIL_LOG=sail_lsp::latency=debug')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    report = json.dumps(summarize(args.log.read_text()), indent=2) + '\n'
    if args.output:
        args.output.write_text(report)
    else:
        print(report, end='')
