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
	cargo bench -p dokar --locked
bench-smoke:
	cargo bench -p dokar --locked -- --test
bench-prepare:
	$(PYTHON) tools/bench.py prepare
bench-transport:
	$(PYTHON) tools/bench.py run
