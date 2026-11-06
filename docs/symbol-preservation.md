# Native debug preservation

`make package` builds release Rust with full debug information and no Cargo
stripping. ELF uses unsplit DWARF; macOS uses Cargo's packed dSYM, emitted while
the final link's temporary Rust objects still exist. Linux cdylibs explicitly request a reproducible
GNU build ID. The staged dynamic library is the final linked module: preserve its
ELF peer or Mach-O dSYM, verify identity/architecture/content, then strip the
staged copy before assembling classifier JARs and the unsigned Maven ZIP.
Packaging never rewrites code addresses after extraction. Later Maven signing
signs the archives, not the native module.

`build/symbols/` is a separate CI artifact (`native-debug-<platform>`, 90 days).
Its records bind exact distributed binary hashes to retained peers, executable
section addresses/bytes, GNU debuglink CRCs, and DWARF inspection diagnostics.
`build/reports/symbol-coverage.json` includes these relationships.

Native and ThinLTO static classifiers retain debug information for their future
consumer links. MYNA transforms ThinLTO IR before those final links; static
archive/debug presence does not establish any final-address contribution. The
consumer must preserve debug information again after its final link/rewrite.

Missing companions and mismatched/stale peers fail preservation. LLVM verifier
errors from optimized producers are retained as explicit quality gaps. This is
artifact preservation, not certification of complete inline attribution or an
online symbolication service. Source publication and guest/JIT capture are
separate work.
