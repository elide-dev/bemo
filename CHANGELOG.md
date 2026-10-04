# Changelog

## [0.2.0](https://github.com/elide-dev/dokar/compare/v0.1.0...v0.2.0) (2026-10-04)


### Features

* extract native transport core and preserve Elide C ABI ([8eb55a2](https://github.com/elide-dev/dokar/commit/8eb55a22552e139f881452b971d4ff6fef44569a))
* migrate Netty transport bindings and verify JVM distributions ([d99f693](https://github.com/elide-dev/dokar/commit/d99f6937892ff44177a5c64f358d3b3125138948))
* unify Dokar namespaces and load native libraries from resources ([7aa1b7b](https://github.com/elide-dev/dokar/commit/7aa1b7bb74e89f211c63f148b85e59143ad78c8c))


### Bug Fixes

* address cross-platform transport CI failures ([f6c4627](https://github.com/elide-dev/dokar/commit/f6c46275a470965e589706ca7f8ed8e63aa505ba))
* **bench:** exclude generated Criterion reports from source fingerprints ([30ae12e](https://github.com/elide-dev/dokar/commit/30ae12e27a5b2fc2260c06b648df1a110dc3fbfb))
* **ci:** complete Linux lint coverage and bootstrap benchmark Rust ([4ccfa44](https://github.com/elide-dev/dokar/commit/4ccfa44e57c73f2c2e87d00671d0252cd3774f72))
* omit Unix cancel replacement on Windows ([ad42a64](https://github.com/elide-dev/dokar/commit/ad42a64d02f7e8ab581252936a09e0b95e0bbd72))
* **release:** verify immutability without an admin-only settings API ([47eaab6](https://github.com/elide-dev/dokar/commit/47eaab6a5956ed65b51d04915fe10014055b8bdb))
* **test:** authenticate pending TLS close after H2 shutdown ([857eb96](https://github.com/elide-dev/dokar/commit/857eb96a45a97619b1752ecf34c08367965f65a0))
