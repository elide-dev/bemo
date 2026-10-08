#!/usr/bin/env python3
"""Measure warmed JVM SSLEngine encryption against tcnative/BoringSSL with a common JSSE peer."""
import argparse
import hashlib
import json
import os
import subprocess
import build


def main():
  parser = argparse.ArgumentParser(description=__doc__)
  parser.add_argument('--warmup', type=int, default=1000)
  parser.add_argument('--rounds', type=int, default=2000)
  parser.add_argument('--samples', type=int, default=3)
  parser.add_argument('--providers', nargs='+', choices=('bemo', 'openssl'), default=['bemo', 'openssl'])
  parser.add_argument('--size', type=int, nargs='+', default=[65536, 131072])
  args = parser.parse_args()
  if min(args.warmup, args.rounds, args.samples, *args.size) < 1 or max(args.size) > 131072:
    parser.error('Positive workloads up to 128 KiB required')
  library = build.library(release=True)
  if not library.is_file():
    raise RuntimeError('Release library required; run make bench-prepare first')
  output = build.BUILD / 'bench/record-classes'
  cp = [build.classes(module) for module in ('api', 'ffm', 'netty')] + build.benchmark_netty()
  build.compile_java(output, [build.ROOT / 'benchmarks/java/TlsRecordBenchmark.java'], cp)
  java = os.environ.get('BEMO_BENCH_JAVA', str(build.java_tool('java')))
  results = []
  for size in args.size:
    for sample in range(args.samples):
      for provider in (args.providers if sample % 2 == 0 else args.providers[::-1]):
        command = [java, '-Xms256m', '-Xmx256m', '--enable-native-access=ALL-UNNAMED', '-cp',
                   build.classpath([output, *cp]), 'TlsRecordBenchmark', str(library),
                   str(build.ROOT / 'crates/bemo/tests/fixtures/localhost-cert.pem'),
                   str(build.ROOT / 'crates/bemo/tests/fixtures/localhost-key.pem'),
                   provider, str(size), str(args.warmup), str(args.rounds)]
        completed = subprocess.run(command, capture_output=True, text=True)
        if completed.returncode:
          raise RuntimeError(completed.stderr)
        result = json.loads(completed.stdout.strip().splitlines()[-1])
        result['sample'] = sample
        results.append(result)
        print(json.dumps(result), flush=True)
  report = {'scope': 'JVM SSLEngine encryption only; binding overhead included; common JSSE peer outside timer',
            'java_version': subprocess.run([java, '-version'], capture_output=True, text=True, check=True).stderr.strip(),
            'library_sha256': hashlib.sha256(library.read_bytes()).hexdigest(),
            'samples': results}
  path = build.BUILD / 'reports/benchmarks/tls-records.json'
  path.parent.mkdir(parents=True, exist_ok=True)
  path.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
  main()
