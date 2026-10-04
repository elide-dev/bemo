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
	cargo test --workspace --all-targets --locked
	cargo test --workspace --doc --locked
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
