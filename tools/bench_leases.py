#!/usr/bin/env python3
"""Qualify ownership microbenchmarks at two pinned commits on one Linux runner."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CASES = ('buffer/retain-drop/4096', 'buffer/slice-drop/4096', 'buffer/exclusive-freeze-recover')
FILTER = 'buffer/(retain-drop/4096|slice-drop/4096|exclusive-freeze-recover)'


def measurements(log):
  pattern = r'(buffer/(?:retain-drop/4096|slice-drop/4096|exclusive-freeze-recover))\s+time:\s+\[([\d.e+-]+) (ns|us|µs) ([\d.e+-]+) (ns|us|µs) ([\d.e+-]+) (ns|us|µs)\]'
  values = []
  for match in re.finditer(pattern, log):
    case, lower, lower_unit, point, point_unit, upper, upper_unit = match.groups()
    factor = {'ns': 1, 'us': 1000, 'µs': 1000}
    values.append({'case': case, 'lower_ns': float(lower) * factor[lower_unit],
                   'point_ns': float(point) * factor[point_unit], 'upper_ns': float(upper) * factor[upper_unit]})
  if len(values) != len(CASES) or {value['case'] for value in values} != set(CASES):
    raise ValueError('Incomplete ownership measurements')
  return values


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--before', required=True)
  parser.add_argument('--after', required=True)
  args = parser.parse_args()
  if sys.platform != 'linux':
    parser.error('Use the shared Linux CI host for qualification')
  output = ROOT / 'build/reports/lease-paired'
  output.mkdir(parents=True, exist_ok=False)
  commits = {name: subprocess.check_output(['git', 'rev-parse', '--verify', '--end-of-options', f'{ref}^{{commit}}'], cwd=ROOT, text=True).strip()
             for name, ref in [('before', args.before), ('after', args.after)]}
  report = {'commits': commits, 'cpu': subprocess.check_output(['lscpu'], text=True),
            'affinity': sorted(os.sched_getaffinity(0)), 'warmup_seconds': 1,
            'measurement_seconds': 2, 'criterion_samples': 30, 'samples': []}
  with tempfile.TemporaryDirectory(prefix='bemo-lease-pair-') as temporary:
    trees = {}
    try:
      for name, commit in commits.items():
        tree = Path(temporary) / name
        subprocess.run(['git', 'worktree', 'add', '--detach', str(tree), commit], cwd=ROOT, check=True)
        trees[name] = tree
        environment = {**os.environ, 'CARGO_TARGET_DIR': str(Path(temporary) / (name + '-target'))}
        (output / name).mkdir()
        with (output / name / 'build.log').open('w') as log:
          subprocess.run(['cargo', 'bench', '--no-run', '--locked', '-p', 'bemo', '--bench', 'buffers'], cwd=tree, env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
      for sample in range(3):
        versions = list(trees) if sample % 2 == 0 else list(reversed(trees))
        for name in versions:
          print(f'Measuring {name}, ownership sample {sample + 1}/3', flush=True)
          environment = {**os.environ, 'CARGO_TARGET_DIR': str(Path(temporary) / (name + '-target'))}
          result = subprocess.run(['cargo', 'bench', '--locked', '-p', 'bemo', '--bench', 'buffers', '--', FILTER,
                                   '--warm-up-time', '1', '--measurement-time', '2', '--sample-size', '30'],
                                  cwd=trees[name], env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=True)
          (output / name / f'{sample}.log').write_text(result.stdout)
          report['samples'].extend({**value, 'version': name, 'sample': sample} for value in measurements(result.stdout))
      (output / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
    finally:
      for tree in trees.values():
        subprocess.run(['git', 'worktree', 'remove', '--force', str(tree)], cwd=ROOT, check=True)


if __name__ == '__main__':
  main()
