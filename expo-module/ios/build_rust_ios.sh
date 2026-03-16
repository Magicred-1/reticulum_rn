#!/usr/bin/env bash
# Add this as an Xcode "Run Script" build phase BEFORE "Compile Sources".
# Runs only when the Rust source is newer than the built lib, so clean builds
# are fast after the first one.
set -euo pipefail

RUST_DIR="${SRCROOT}/../rust-core"
LIB_NAME="libreticulum_mobile.a"
OUTPUT_DIR="${SRCROOT}/Frameworks"

mkdir -p "${OUTPUT_DIR}"

# Install targets once (CI will already have them)
rustup target add aarch64-apple-ios x86_64-apple-ios aarch64-apple-ios-sim 2>/dev/null || true

cd "${RUST_DIR}"

# Build for physical device
cargo build --release --target aarch64-apple-ios

# Build for simulator (x86 + ARM)
cargo build --release --target x86_64-apple-ios
cargo build --release --target aarch64-apple-ios-sim

# Create a fat simulator lib
lipo -create \
    "target/x86_64-apple-ios/release/${LIB_NAME}" \
    "target/aarch64-apple-ios-sim/release/${LIB_NAME}" \
    -output "target/sim-fat/${LIB_NAME}"

# XCFramework bundles both slices — Xcode picks the right one automatically
xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/release/${LIB_NAME}" \
    -library "target/sim-fat/${LIB_NAME}" \
    -output "${OUTPUT_DIR}/ReticulumMobile.xcframework"

echo "✓ Rust → ReticulumMobile.xcframework"
