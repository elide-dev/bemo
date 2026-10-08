PYTHON ?= python3
ELIDE ?= elide
export ELIDE

.PHONY: help deps build build-rust build-jvm test test-rust test-jvm test-native-image check fmt fmt-check package clean
help:
	@$(PYTHON) tools/build.py --help
deps:
	$(PYTHON) tools/build.py deps
build:
	$(PYTHON) tools/build.py build
build-rust:
	cargo build --workspace --locked
build-jvm:
	$(PYTHON) tools/build.py jvm
test:
	$(PYTHON) tools/build.py test
test-rust:
	$(PYTHON) tools/build.py test-rust
test-jvm:
	$(PYTHON) tools/build.py test-jvm
test-native-image:
	$(PYTHON) tools/build.py test-native-image
check:
	$(PYTHON) tools/build.py check
fmt:
	$(PYTHON) tools/build.py fmt
fmt-check:
	$(PYTHON) tools/build.py fmt-check
package:
	$(PYTHON) tools/build.py package
clean:
	$(PYTHON) tools/build.py clean

.PHONY: coverage coverage-rust coverage-jvm
coverage: coverage-rust coverage-jvm
coverage-rust:
	$(PYTHON) tools/build.py coverage-rust
coverage-jvm:
	$(PYTHON) tools/build.py coverage-jvm

.PHONY: bench bench-smoke bench-prepare bench-transport
bench:
	cargo bench -p bemo --locked
bench-smoke:
	cargo bench -p bemo --locked -- --test
bench-prepare:
	$(PYTHON) tools/bench.py prepare
bench-transport:
	$(PYTHON) tools/bench.py run

.PHONY: bench-compression
bench-compression:
	$(PYTHON) tools/compression_probe.py

.PHONY: bench-graphs
build/chart-venv/.deps: tools/chart-requirements.txt
	$(PYTHON) -m venv build/chart-venv
	build/chart-venv/bin/python -m pip install -r tools/chart-requirements.txt
	touch $@
bench-graphs: build/chart-venv/.deps
	build/chart-venv/bin/python tools/plot_bench.py $(CHART_ARGS)

.PHONY: fuzz fuzz-smoke test-asan test-tsan test-miri
fuzz:
	$(PYTHON) tools/verify.py fuzz
fuzz-smoke:
	$(PYTHON) tools/verify.py fuzz --runs 1000
test-asan:
	$(PYTHON) tools/verify.py asan
test-tsan:
	$(PYTHON) tools/verify.py tsan
test-miri:
	$(PYTHON) tools/verify.py miri

.PHONY: generate-seam build-bitcode test-bitcode
generate-seam:
	$(PYTHON) tools/seam.py
build-bitcode:
	$(PYTHON) tools/bitcode.py build
test-bitcode:
	$(PYTHON) tools/bitcode.py test

.PHONY: examples-prepare examples-build test-examples
examples-prepare: build
	$(PYTHON) tools/examples.py prepare
examples-build: examples-prepare
	$(PYTHON) tools/examples.py build
test-examples: examples-prepare
	$(PYTHON) tools/examples.py test

.PHONY: examples-native test-examples-native
examples-native: examples-prepare
	$(PYTHON) tools/examples.py native
test-examples-native: examples-prepare
	$(PYTHON) tools/examples.py test --native

.PHONY: bench-tls-records
bench-tls-records:
	$(PYTHON) tools/tls_records.py
