#!/usr/bin/env python3
"""Measure two commits sequentially on one Linux host, alternating version and transport order."""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile

import bench

ROOT = Path(__file__).resolve().parents[1]


def aggregate(rows):
  groups = {}
  for row in rows:
    key = (row['case'], row['transport'])
    group = groups.setdefault(key, {**row, 'samples': []})
    group['samples'].extend(row['samples'])
  for row in groups.values():
    samples = row['samples']
    row.update(
        median_requests_per_second=statistics.median(s['requests_per_second'] for s in samples),
        median_latency_p99_ns=statistics.median(s['latency_p99_ns'] for s in samples),
        median_cpu_ns_per_request=statistics.median(s['process_cpu_ns'] / s['requests'] for s in samples),
        median_server_cpu_ns_per_request=statistics.median(s['server_cpu_ns'] / s['requests'] for s in samples),
        median_client_cpu_ns_per_request=statistics.median(s['client_cpu_ns'] / s['requests'] for s in samples),
        max_peak_rss_bytes=max(s['peak_rss_bytes'] for s in samples))
  return list(groups.values())


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--before', required=True)
  parser.add_argument('--after', required=True)
  parser.add_argument('--rounds', type=int, default=25000)
  parser.add_argument('--warmup', type=int, default=5000)
  parser.add_argument('--samples', type=int, default=3)
  parser.add_argument('--output', type=Path, default=ROOT / 'build/reports/readme-paired')
  args = parser.parse_args()
  if sys.platform != 'linux':
    parser.error('Paired README measurements require Linux RSS and io_uring/epoll')
  if min(args.rounds, args.warmup) < 1 or args.samples < 3:
    parser.error('Positive work and at least three samples are required')
  if os.environ.get('CARGO_TARGET_DIR'):
    parser.error('Separate Cargo target directories are required to preserve prepared artifacts')
  output = args.output.resolve()
  output.mkdir(parents=True, exist_ok=False)
  commits = {name: subprocess.check_output(['git', 'rev-parse', '--verify', '--end-of-options', f'{ref}^{{commit}}'],
                                          cwd=ROOT, text=True).strip()
             for name, ref in [('before', args.before), ('after', args.after)]}
  environment = {
    'commits': commits,
    'runner_cpu': subprocess.check_output(['lscpu'], text=True),
    'affinity': sorted(os.sched_getaffinity(0)),
    'rounds': args.rounds, 'warmup_rounds': args.warmup, 'samples': args.samples,
    'clients': 4, 'version_order': 'alternates by case and sample',
    'transport_order': 'alternates by sample',
  }
  (output / 'environment.json').write_text(json.dumps(environment, indent=2) + '\n')
  with tempfile.TemporaryDirectory(prefix='bemo-bench-pair-') as temporary:
    worktrees = {}
    try:
      for name, commit in commits.items():
        worktree = Path(temporary) / name
        subprocess.run(['git', 'worktree', 'add', '--detach', str(worktree), commit], cwd=ROOT, check=True)
        worktrees[name] = worktree
        print(f'Preparing {name}: {commit}', flush=True)
        subprocess.run(['make', 'bench-prepare', f'PYTHON={sys.executable}'], cwd=worktree, check=True)
        (output / name / 'samples').mkdir(parents=True)
      rows = {name: [] for name in worktrees}
      cases = list(bench.CASES)
      for case_index, case in enumerate(cases):
        for sample in range(args.samples):
          versions = list(worktrees)
          if (case_index + sample) % 2:
            versions.reverse()
          transports = 'epoll,bemo' if sample % 2 else 'bemo,epoll'
          for name in versions:
            worktree = worktrees[name]
            print(f'Measuring {case}, sample {sample + 1}/{args.samples}, {name}, {transports}', flush=True)
            subprocess.run([sys.executable, 'tools/bench.py', 'run', '--case', case,
                            '--rounds', str(args.rounds), '--warmup', str(args.warmup),
                            '--samples', '1', '--transports', transports], cwd=worktree, check=True)
            source = worktree / f'build/reports/benchmarks/summary-{case}.json'
            measured = json.loads(source.read_text())
            assert {s['commit'] for row in measured for s in row['samples']} == {commits[name]}
            assert {row['transport'] for row in measured} == {'bemo', 'epoll'}
            assert all(len(row['samples']) == 1 and row['case'] == case for row in measured)
            rows[name].extend(measured)
            (output / name / 'samples' / f'{case}-{sample}.json').write_bytes(source.read_bytes())
      for name, measured in rows.items():
        summary = aggregate(measured)
        assert len(summary) == 16 and all(len(row['samples']) == args.samples for row in summary)
        (output / name / 'summary-all.json').write_text(json.dumps(summary, indent=2) + '\n')
        (output / name / 'summary-all-comparison.json').write_text(
            json.dumps(bench.comparisons(summary), indent=2) + '\n')
    finally:
      for worktree in worktrees.values():
        subprocess.run(['git', 'worktree', 'remove', '--force', str(worktree)], cwd=ROOT, check=True)
  print(output.relative_to(ROOT) if output.is_relative_to(ROOT) else output, flush=True)


if __name__ == '__main__':
  main()
