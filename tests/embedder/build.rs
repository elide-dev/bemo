fn main() {
  // Embedders read Bemo's headers from its `links = "bemo"` metadata, never a copy.
  let directory = std::env::var("DEP_BEMO_INCLUDE").expect("bemo-ffi exports DEP_BEMO_INCLUDE");
  for header in ["bemo.h", "elide_transport.h"] {
    let path = std::path::Path::new(&directory).join(header);
    assert!(path.is_file(), "{header} missing from DEP_BEMO_INCLUDE");
  }
}
