// The reticulum crate uses prost/tonic for the Kaonic gRPC interface.
// This build.rs is a passthrough — the reticulum crate's own build.rs
// handles proto compilation. We include this file so Cargo doesn't warn
// about a missing build script when build = true is inferred.
fn main() {}
