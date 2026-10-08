#!/usr/bin/env bash
set -euo pipefail
cd "$HOME/bemo-native-20261007"
export ELIDE="$HOME/.local/share/mise/installs/github-elide-dev-elide/1.5.4+20260921/bin/elide"
export JAVA_HOME="$HOME/.local/share/mise/installs/java/openjdk-25.0.2"
export PATH="$JAVA_HOME/bin:$HOME/.local/share/mise/installs/github-elide-dev-elide/1.5.4+20260921/lib/svm/bin:$HOME/.cargo/bin:$PATH"
export BEMO_BENCH_JAVA="$JAVA_HOME/bin/java"
export BEMO_BENCH_GZIP_LEVEL=1
export BEMO_BENCH_SERVER_CPUS=12-17
export BEMO_BENCH_CLIENT_CPUS=18-21
mkdir -p build/reports
trap 'echo "exit=$?" > build/reports/cycle.status' EXIT
step() {
  echo "$1" > build/reports/cycle.phase
  local phase="$1"
  shift
  "$@" > "build/reports/cycle-$phase.log" 2>&1
}
step check make check
step test make test
step native-contract make test-native-image
step git-consumer python3 tools/test_git_dependency.py
step example-prepare python3 tools/examples.py prepare
step example-build python3 tools/examples.py native --native-opt 3
step example-jvm-test python3 tools/examples.py smoke
step example-native-test python3 tools/examples.py smoke --native
step bench-prepare make bench-prepare
python3 - <<'PY'
import hashlib, json, subprocess
from pathlib import Path
names = subprocess.check_output(['git', 'ls-files'], text=True).splitlines()
manifest = {'parent_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
            'working_tree': subprocess.check_output(['git', 'status', '--short'], text=True),
            'source_files': {name: hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in names if Path(name).is_file()},
            'cycle_script_sha256': hashlib.sha256(Path('build/run-cycle.sh').read_bytes()).hexdigest()}
Path('source-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
PY
step sizes python3 tools/bench_sizes.py
step backend-probe python3 tools/bench_frameworks.py --probe --workload tls-compression --server-cpus 12-17 --client-cpus 18-21 --output build/reports/framework-backend-probe
step criterion make bench
step basics python3 tools/bench.py run --backend 2
for workload in plaintext payload compression tls tls-compression; do
  step "framework-$workload" python3 tools/bench_frameworks.py --workload "$workload" --server-cpus 12-17 --client-cpus 18-21 --output "build/reports/framework-$workload"
done
step compression python3 tools/compression_probe.py --backends zlib-rs zlib zlib-ng
 echo complete > build/reports/cycle.phase
