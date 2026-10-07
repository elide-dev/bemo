# Generated Native Image imports and ThinLTO libraries

Bemo uses a pinned [Myna](https://github.com/elide-dev/myna) revision to
produce all 90 raw Native Image C imports. `seams/bemo.seam` includes both
callback signatures and references Bemo’s public function-pointer interfaces.
Myna emits `@CContext` and `@CLibrary` directly from the descriptor, using
Bemo’s header directives. Existing Java adapters retain handle ownership,
lifecycle checks, callback dispatch, entry-point literals, and exception
translation. Generation requires no annotation rewriting. Rust callback contexts
use `*mut c_void`, matching the existing `void *` C contract; Rust callers pass
context pointers and use `null_mut()` for a null context.

## Generation

```sh
make generate-seam
make build
make check
make test
make test-native-image
```

The build obtains the generator revision recorded in `tools/versions.json`.
It compiles only the dependency-free generator CLI’s tracked sources through
Elide, then checks the
committed `BemoNatives.java` against fresh generation. C signatures, Rust export
signatures, callback function-pointer types, and record layouts must agree.
The generated Java source is included in the ordinary sources JAR.

For development, set `MYNA_HOME` to a clean checkout at the recorded revision.
When updating the generator, fast-forward that checkout, review the changed
interfaces, update the revision in `tools/versions.json`, run
`make generate-seam`, and rerun the contracts. Never move CI onto an unpinned
branch. The generator output has no runtime dependency on Myna.

Common Java artifacts use the canonical `aarch64-apple-darwin` descriptor.
The generated Java `ABI_FINGERPRINT` identifies that canonical descriptor;
it is not the target fingerprint of an extracted library. All currently
supported seam targets have the same scalar widths and record layouts.
Native classifiers carry the actual target's `seam.json`, `seam.abi`, and
`seam.ll`. This keeps common Java JARs identical across platform builds.

## Bitcode archive variant

```sh
make build-bitcode
make test-bitcode
make package
python3 tools/verify_package.py
```

The additional Maven classifier is `<platform>-thinlto` on
`dev.elide.bemo:bemo-native-image`. It contains:

- `libbemo_ffi_thinlto.a`, a regular static archive containing LLVM bitcode and
  required native members;
- the public headers, target-specific seam JSON, fingerprint and LLVM contracts;
- a manifest with archive digest, member
  coverage, compiler versions, flags, and generator revision.

Resources live under `META-INF/native/<platform>/`, as in the ordinary native
classifiers. Extract them together. The bitcode archive is an additional
variant; ordinary static and shared classifiers retain their names.

Rust dependencies use linker-plugin LTO in a separate Cargo `bitcode` profile
and target directory. C dependencies can inherit ThinLTO through their build
systems. Assembly and prebuilt Rust standard-library objects remain native;
the manifest reports every member rather than claiming full IR coverage.
Use compatible LLVM tools for building, application, and final linkage.
`LLVM_BIN` selects the tool directory; executables missing there are looked up
on `PATH` (for example, Homebrew's separately installed LLD).
The build checks Clang, LLD, and LLVM helper libraries against rustc's LLVM
version. Package CI downloads the exact LLVM 23.1.1 distributions and validates
the SHA-256 digests recorded in `tools/versions.json`; `tools/setup_llvm.py`
can provision the same distributions locally. It uses LLVM archiving tools for dependency objects: Apple's archiver
can silently drop bitcode members whose architecture it cannot read.

For a C consumer, after extracting the classifier to a directory:

```sh
clang -O2 -flto=thin -fuse-ld=lld -I extracted \
  consumer.c extracted/libbemo_ffi_thinlto.a -o consumer
```

On Linux, add `-ldl -lpthread -lm`. This archive requires an LLVM LTO-capable
linker. The name has a `_thinlto` suffix so an ordinary build cannot select it
accidentally. For a Native Image consumer whose C library configuration names
`bemo_ffi`, stage this variant under `libbemo_ffi.a` in a dedicated library
search directory and configure the Native Image final link to use the matching
LLVM toolchain and ThinLTO. The ordinary Native Image contracts continue to
exercise the ordinary static library; the variant's C consumer independently
proves its linker path. Merely pointing Native Image at bitcode without an
LTO-capable linker is insufficient.

`Myna`'s LLVM helper applies the generated contracts to the actual bitcode
archive before consumer indexing, preserving native members and rebuilding the
archive index and modified module summaries. The initial descriptor makes no
new LLVM optimizer promises. Existing reviewed Native Image leaf-call policies
are recorded separately from LLVM facts. Apply additional contracts only when
the implementation satisfies them. Consumers applying contracts to their own
callers must likewise rewrite fresh bitcode before linking; adding `seam.ll`
as a separate declaration module does not annotate unrelated calls.

Package verification extracts the staged variant, validates its digest and
metadata, links/runs the ownership contract through ThinLTO, and inspects saved
optimized IR for an actual Bemo definition. CI runs this on Linux x86_64 and
macOS ARM64. Local verification establishes the host platform; CI establishes
its own platforms when run.

## Fat objects

The shipped variant uses standalone bitcode archive members. LLVM's genuine
`-ffat-lto-objects` container currently supports ELF, while the macOS package
uses Mach-O. Rust's full/fat LTO optimization mode is distinct from a native
object with an embedded LTO representation. Separate native and bitcode
archives provide both consumption paths without claiming a portable fat
container. See [LLVM FatLTO](https://llvm.org/docs/FatLTO.html) and
[Rust linker-plugin LTO](https://doc.rust-lang.org/rustc/linker-plugin-lto.html).
