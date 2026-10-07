# Generated Native Image seam and LLVM bitcode archives

## Intent and approved scope

Adopt `elide-dev/svmgen` to generate Bemo's Native Image C API imports and
provide static libraries that LLVM consumers can use with ThinLTO. The user
approved incremental adoption and accepts either genuine fat archives or
separate archives containing LLVM bitcode. Refresh the local generator checkout
regularly during implementation because its supported shapes are changing.

Success means the generated imports execute the existing transport contracts,
the packaged bitcode archive links and runs in a real ThinLTO consumer, and the
ordinary static/shared libraries continue to pass their existing contracts.

## Boundaries

- Cargo owns Rust; Elide owns Java compilation, dependency resolution, and JARs.
- `crates/bemo` stays independent of JVM, Netty, GraalVM, and Elide runtime types.
- Unmangled exports stay in `crates/bemo-ffi`; `bemo::abi` owns handle operations.
- Shared consumer headers stay in `include`.
- Preserve Bemo ABI 1 and transport ABI 3, symbols, record layouts, and handle
  semantics. This migration introduces no data-plane capability.
- API/FFM artifacts acquire no GraalVM or Elide runtime dependencies.
- Use 2-space indentation, LF, Cargo fmt, and google-java-format.
- Keep engineering documentation in `docs`; do not hardcode local paths.
- Stage and verify packages locally. Publishing is a separate operation.

## Generator acquisition and refresh

Record a repository URL and exact generator revision in Bemo's tool metadata.
Support a configurable local checkout or executable override for development;
resolve defaults relative to the repository or through executable discovery.
CI must use the recorded revision rather than a moving branch.

At the start of each implementation phase, inspect the local generator worktree
and fast-forward its tracked branch when clean. Preserve upstream/local edits;
never reset or discard them. Re-read affected generator interfaces after an
update. Rebuild stale generator outputs and advance Bemo's recorded revision
only after integration checks pass. Record the actual generator revision in
package provenance when an override is used.

Initial inspected revision: `6892225acaa1b7fef0e7a3496104195115fae94d`.
The first requested refresh returned already up to date. The next refresh
advanced to `3eaf2950df92f3f03f501be289e669b13136a16e`, adding Java naming
configuration. Subsequent refreshes initially found no changes; the final-phase refresh
advanced to svmgen 0.2.0 at `1bde5a11b1f25446e6410c27e6484546b4379c7d`, adding typed Native
Image carriers. Regenerate and verify those carriers before shipping.

## Descriptor and generated imports

Add a checked-in seam descriptor for the Bemo queries and transport imports.
Use exact existing symbols, scalar widths, pointer shapes, record offsets,
nullability, and ownership. Distinguish nullable empty buffers from pointers
that must refer to valid output storage. Preserve existing transition choices
only after checking their implementations against the generator's leaf-call
obligations. Do not infer optimizer attributes from C constness or Java method
bodies. Functions that block, allocate unpredictably, or call Java retain the
normal transition.

Generate Java raw imports, JSON metadata, ABI fingerprints, and LLVM contracts
before Java compilation. Generation must be deterministic, target-explicit,
and checked for drift. Keep the two existing `CapiTransportNative` classes as
public compatibility adapters, delegating to generated imports. Translate
addresses to the generated pointer carriers at this adapter boundary.

The refreshed generator supports module-level Java package/class naming.
Use `java_package=dev.elide.bemo.svm.generated` and `java_class=BemoNatives`
directly. Add C context/library annotations through a narrowly scoped,
validated adaptation until upstream supplies them. Include generated Java in
source JARs and Javadoc inputs, not only compiled classes.

Generate common Java using the canonical `aarch64-apple-darwin` descriptor
so its embedded fingerprint and all common JARs remain identical across
platforms. This fingerprint describes the canonical descriptor. Generate
classifier JSON/fingerprints/LLVM contracts for the actual consumer target.

Keep reviewed human-facing headers and the existing Rust forwarding-export
generator. Compare descriptor signatures and generated record layouts with
those headers and exports so they cannot drift independently. Generated C
headers can serve as validation artifacts without replacing documented public
headers and constants.

## Callback boundary

The initial upstream generator does not support typed function pointers.
Keep callback imports and Java callback entrypoints handwritten, with an
explicit inventory of their signatures and transition requirements. Preserve
the isolate-thread context, event batching, reentry rules, and exception
translation. Do not disguise callback pointers as data pointers to obtain
nominal descriptor coverage.

Generate every supported non-callback import. Verify complete symbol coverage
as the union of generated imports and the explicit callback inventory, checking
that the sets do not overlap. If refreshed upstream adds callback support,
adopt it only with equivalent ABI and lifecycle evidence.

## Archive variants

Retain the existing ordinary static archive and shared library. Add a separate
`libbemo_ffi_thinlto.a` archive containing Rust linker-plugin-LTO bitcode and any
required native archive members. Use a dedicated Cargo target directory/profile
so bitcode builds cannot overwrite ordinary outputs or poison Cargo caches.
Build only the staticlib for this variant: the package currently also requests
rlib/cdylib outputs, and an unqualified workspace build is unsuitable here.

Use the pinned Rust compiler with `-Clinker-plugin-lto`, compatible Clang and
LLVM tools, and a compatible final linker. The initial local Rust and Clang
report LLVM 23.1.1. Check toolchain compatibility explicitly and prove it by
linking the actual archive; matching major versions alone is insufficient.

Inventory archive members. Bemo and its Rust dependency objects must provide
usable bitcode. Allocator, TLS, assembly, and Rust standard-library members may
remain native where required; report that coverage accurately in the manifest.
For native C dependencies, use compatible ThinLTO compilation when their build
systems support it. Never claim every archive member is LLVM IR.

Stage an additional Native Image classifier with the existing platform
classifier plus `-thinlto`, containing the variant, public headers, seam JSON,
fingerprint, LLVM contract file, and a manifest. The manifest records target,
Rust/LLVM/generator versions, relevant build flags, and member coverage.
Ordinary classifiers retain their current names and extraction behavior.
Checksums and local release merging must include the additional classifier.

Genuine fat archives are an optional replacement for the extra variant on
supported targets only after both ordinary and ThinLTO links are demonstrated.
LLVM FatLTO currently supports ELF; do not promise the same container on
Mach-O. Rust's full/fat LTO mode is a different concept from fat object
containers. Never construct a fat object by pairing unrelated native code and
IR, or by mixing duplicate native/bitcode definitions as archive members.

## Contract application

Use `svmgen apply` on fresh bitcode inputs before ThinLTO indexing when applying
approved optimizer facts. Attach facts to actual definitions and direct call
sites; linking a declaration-only `seam.ll` file does not establish them in
other modules. Keep original compiler outputs untouched and use separate
annotated paths. Contract bytes and generator/helper identities participate in
build invalidation.

Ship contracts for consumers even when the initial descriptor carries no
additional optimizer facts. Do not manufacture optimization assertions to
make the pipeline appear effective. The initial applicator does not support
embedded-bitcode containers; fat packaging must account for this if selected.

## Failure behavior

Generation fails on unsupported shapes outside the callback inventory,
signature/layout mismatch, stale generated files, or incompatible generator
output. Variant builds fail clearly on a missing or incompatible toolchain.
Package verification fails on absent metadata, wrong target, unusable archive
members, or failure of the real consumer link. No fallback may silently label
an ordinary native-only archive as a ThinLTO variant.

## Verification and acceptance

1. Check descriptor/header/Rust signatures, target-specific layout assertions,
   generated artifact freshness, and explicit callback coverage.
2. Run `make fmt`, `make build`, `make check`, `make test`, and
   `make test-native-image`, plus `python3 tools/test_git_dependency.py`.
3. Exercise both Java bindings through the shared contract, including callback
   reentry, lifecycle, pointer ownership, TLS, and error paths.
4. Stage with `make package` and run package verification. Verify generated
   sources are published, API/FFM dependency isolation remains intact, and
   ordinary static/shared consumers still run.
5. Inspect variant member contents, link a C consumer against the staged
   archive with ThinLTO, run it, and inspect saved linker IR for Bemo definitions
   participating in optimization. Merely finding a `.bc` filename is inadequate.
6. Exercise Linux and macOS supported CI platforms; use appropriate linker
   drivers for each. If fat archives are selected, also run a non-LTO consumer
   against the identical staged archive and verify embedded bitcode selection.

## References

- [Rust linker-plugin LTO](https://doc.rust-lang.org/rustc/linker-plugin-lto.html)
- [LLVM FatLTO](https://llvm.org/docs/FatLTO.html)
- Generator repository: `https://github.com/elide-dev/svmgen`, particularly
  `docs/dsl.md` and `docs/thinlto.md` at the recorded revision.
