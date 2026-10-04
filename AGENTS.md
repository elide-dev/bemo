# Working on Dokar

- Use `make build`, `make check`, `make test`, and `make test-native-image`.
  Cargo owns Rust; Elide owns Java compilation, dependency resolution, and JARs.
- Keep `crates/dokar` independent of JVM, Netty, GraalVM, and Elide runtime types.
  Unmangled C exports belong only in `crates/dokar-ffi`; shared headers live in
  `include`. `dokar::abi` owns the Rust handle operations behind those exports.
- ABI changes must update both Java bindings and run their shared contract.
  Never advertise a capability before its data plane and ownership tests exist.
- API/FFM artifacts must have no GraalVM or Elide runtime dependencies.
- Pin Cargo forks in dependency declarations; root-only patches do not travel
  into consuming Cargo workspaces. Run `tools/test_git_dependency.py`.
- Use 2-space indentation, LF, Cargo fmt, and google-java-format (`make fmt`).
- Engineering documentation belongs in `docs/`. Do not hardcode local paths.
- Keep secrets out of PR workflows. Publishing is separate from local staging.
