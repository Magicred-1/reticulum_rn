#!/usr/bin/env bash
# setup.sh — Bootstrap reticulum-rn from scratch.
# Run from the repo root:  bash setup.sh
set -euo pipefail

BOLD='\033[1m'; RESET='\033[0m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'

step() { echo -e "\n${BOLD}▶ $1${RESET}"; }
ok()   { echo -e "${GREEN}✓ $1${RESET}"; }
warn() { echo -e "${YELLOW}⚠ $1${RESET}"; }
fail() { echo -e "${RED}✗ $1${RESET}"; exit 1; }

# ── 0. Check prerequisites ────────────────────────────────────────────────────
step "Checking prerequisites"

command -v rustup  >/dev/null 2>&1 || fail "rustup not found — install from https://rustup.rs"
command -v cargo   >/dev/null 2>&1 || fail "cargo not found"
command -v node    >/dev/null 2>&1 || fail "node not found — install from https://nodejs.org"
command -v protoc  >/dev/null 2>&1 || warn "protoc not found — reticulum crate needs it (brew install protobuf)"

ok "Prerequisites OK"

# ── 1. Rust targets ───────────────────────────────────────────────────────────
step "Installing Rust mobile targets"

rustup target add \
  aarch64-apple-ios \
  aarch64-apple-ios-sim \
  x86_64-apple-ios \
  aarch64-linux-android \
  armv7-linux-androideabi \
  2>/dev/null || true

ok "Rust targets installed"

# ── 2. cargo-ndk (Android) ────────────────────────────────────────────────────
step "Installing cargo-ndk"

if ! command -v cargo-ndk >/dev/null 2>&1; then
  cargo install cargo-ndk
fi
ok "cargo-ndk ready"

# ── 3. Verify Rust core compiles (host target — quick smoke test) ─────────────
step "Smoke-testing rust-core (host target)"

cd rust-core
# Clean first — proc-macro .so files (tokio_macros, etc.) get corrupted on
# interrupted builds. A clean ensures they rebuild from scratch.
cargo clean 2>/dev/null || true
cargo check 2>&1 | tail -5
ok "rust-core compiles"
cd ..

# ── 4. iOS build (skip if not on macOS / Xcode not present) ──────────────────
step "iOS Rust build"

if [[ "$(uname)" == "Darwin" ]] && command -v xcodebuild >/dev/null 2>&1; then
  bash expo-module/ios/build_rust_ios.sh
  ok "ReticulumMobile.xcframework built"
else
  warn "Skipping iOS build (not macOS or Xcode not installed)"
fi

# ── 5. Android build ──────────────────────────────────────────────────────────
step "Android Rust build (arm64-v8a + armeabi-v7a)"

if [[ -n "${ANDROID_NDK_HOME:-}" ]] || [[ -n "${ANDROID_HOME:-}" ]]; then
  cd rust-core
  cargo ndk \
    --target aarch64-linux-android \
    --target armv7-linux-androideabi \
    --output-dir "../expo-module/android/src/main/jniLibs" \
    -- build --release
  ok "Android .so files built"
  cd ..
else
  warn "Skipping Android build (ANDROID_NDK_HOME not set)"
  warn "Set ANDROID_NDK_HOME and re-run, or let Gradle build it automatically"
fi

# ── 6. JS/TS dependencies ─────────────────────────────────────────────────────
step "Installing JS dependencies"

cd expo-module
if command -v pnpm >/dev/null 2>&1; then
  pnpm install
elif command -v yarn >/dev/null 2>&1; then
  yarn install
else
  npm install
fi
ok "JS deps installed"

# ── 7. TypeScript check ───────────────────────────────────────────────────────
step "TypeScript type check"
npx tsc --noEmit 2>&1 | head -20 || warn "Type errors found (see above) — fix before shipping"
cd ..

echo -e "\n${BOLD}${GREEN}Setup complete.${RESET}"
echo ""
echo "Next steps:"
echo "  iOS:     pod install in your Expo app, then open .xcworkspace in Xcode"
echo "  Android: open the android/ folder in Android Studio, or run: npx expo run:android"
echo "  Docs:    see README.md for full integration guide"
