# Porting Reticulum-rs as an Expo / React-Native Module — Best Practices

> Based on a thorough analysis of the existing `reticulum-rn` codebase, the upstream
> [Reticulum-rs](https://github.com/BeechatNetworkSystemsLtd/Reticulum-rs) crate (`0.1.0`),
> and the Expo Modules API conventions.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Rust Core Layer — Best Practices](#2-rust-core-layer--best-practices)
3. [C FFI Layer — Best Practices](#3-c-ffi-layer--best-practices)
4. [JNI Bridge (Android) — Best Practices](#4-jni-bridge-android--best-practices)
5. [iOS Native Module (Swift) — Best Practices](#5-ios-native-module-swift--best-practices)
6. [Android Native Module (Kotlin) — Best Practices](#6-android-native-module-kotlin--best-practices)
7. [TypeScript API Layer — Best Practices](#7-typescript-api-layer--best-practices)
8. [Build System & Toolchain — Best Practices](#8-build-system--toolchain--best-practices)
9. [Integration into anon0mesh — Best Practices](#9-integration-into-anon0mesh--best-practices)
10. [Issues & Improvements Found in Current Codebase](#10-issues--improvements-found-in-current-codebase)
11. [Recommended Migration Checklist](#11-recommended-migration-checklist)

---

## 1. Architecture Overview

The existing codebase follows a **5-layer sandwich architecture** that is well-suited
for embedding a Rust networking library inside an Expo/React-Native app:

```
┌──────────────────────────────────────────────────────┐
│  Layer 5: React Native / TypeScript                   │
│  useMesh() hook  ·  index.ts  ·  MeshScreen.tsx       │
├──────────────────────────────────────────────────────┤
│  Layer 4: Expo Modules API                            │
│  ReticulumModule.swift (iOS)                          │
│  ReticulumModule.kt   (Android)                       │
├──────────────────────────────────────────────────────┤
│  Layer 3: Platform bridges                            │
│  C FFI via bridging header (iOS)                      │
│  JNI via `jni` crate (Android)                        │
├──────────────────────────────────────────────────────┤
│  Layer 2: Rust wrapper / FFI surface                  │
│  ffi.rs  ·  jni_bridge.rs                             │
├──────────────────────────────────────────────────────┤
│  Layer 1: Rust core (reticulum crate 0.1.0)           │
│  node.rs — MeshNode, BLEDriver, LoRaDriver            │
└──────────────────────────────────────────────────────┘
```

**Key design principle**: Rust owns the async event loop (tokio multi-thread runtime),
native modules poll Rust via timer-driven drain loops, and events flow upstream via
Expo's EventEmitter system for the JS layer to consume.

---

## 2. Rust Core Layer — Best Practices

### 2.1 Reticulum-rs API Usage

The `node.rs` already demonstrates the correct usage patterns for the Reticulum-rs
`0.1.0` public API. Key points to maintain:

| Concept | Correct Pattern (from codebase) | Notes |
|---|---|---|
| Identity | `PrivateIdentity::new_from_rand(OsRng)` for keypair | `Identity` is the PUBLIC half only |
| Destinations | `SingleInputDestination` / `PlainInputDestination` | `Destination` is generic — use the aliases |
| Transport | Fully async/tokio, use `Arc<Transport>` | NOT thread + `std::sync` |
| Interface trait | Only requires `fn mtu() -> usize` | Communication via tokio mpsc channels |
| Packet serialization | Manual (private `reticulum::serde`) | `serialize_packet()` replicates wire format |
| Receiving | `transport.iface_rx()` → `broadcast::Receiver<RxMessage>` | Not read()/write() |

### 2.2 MeshNode Design Patterns

The `MeshNode` pattern is well-structured. Best practices observed:

- **Single global instance** via `lazy_static! { NODE: Mutex<Option<MeshNode>> }` —
  prevents double-init and provides thread-safe access from FFI.
- **ByteQueue** (`Arc<Mutex<VecDeque<Vec<u8>>>>`) — clean shared queue between sync FFI
  and async Tokio tasks. This is the right pattern for bridging sync C/JNI callers to
  async Rust.
- **CancellationToken** for graceful shutdown — the `cancel.cancelled()` pattern inside
  `tokio::select!` is idiomatic.
- **Interface drivers** (BLEDriver, LoRaDriver) correctly implement the `Interface` trait
  and communicate via `InterfaceContext::channel.split()` for RX/TX separation.

### 2.3 Recommendations for Improvement

```
CURRENT ISSUE                          RECOMMENDED FIX
─────────────────────────────────────  ─────────────────────────────────────
Identity loaded 3x (new + start +     Cache identity in MeshNode::new() and
add_destination)                       reuse the same PrivateIdentity instance.

Manual packet serialization            Track upstream Reticulum-rs for when
(replicates wire format because        `reticulum::serde` becomes public.
`reticulum::serde` is private)         File an issue upstream if needed.

No error propagation from Tokio        Return Result<(), Error> from start()
runtime spawn failures                 and propagate to FFI layer.

BLEDriver and LoRaDriver are nearly    Extract a generic QueuedDriver<T> that
identical (code duplication)           parameterizes MTU and fragmentation.

tokio runtime with 2 worker threads    Consider 1 thread for constrained
                                       mobile devices; benchmark both.

PeerTable uses std::sync::Mutex        Consider parking_lot::Mutex for lower
inside async context                   contention, or tokio::sync::Mutex.
```

### 2.4 Crate Type Configuration

The current `Cargo.toml` correctly uses dual crate types:

```toml
[lib]
crate-type = ["staticlib", "cdylib"]
```

- **`staticlib`** → iOS (linked into `.xcframework` via `lipo`)
- **`cdylib`** → Android (loaded as `.so` via JNI `System.loadLibrary()`)

This is the correct pattern. Do NOT change to just one.

### 2.5 Dependencies Best Practices

```toml
# ✅ GOOD: Platform-conditional dependencies
[target.'cfg(target_os = "android")'.dependencies]
jni         = { version = "0.21", features = ["invocation"] }
android_log = "0.1"

[target.'cfg(not(target_os = "android"))'.dependencies]
env_logger  = "0.10"

# ✅ GOOD: Explicit tokio features (no full) — prevents bloat
tokio = { version = "1", features = ["rt-multi-thread", "time", "sync", "macros"] }
```

### 2.6 Release Profile

```toml
[profile.release]
opt-level = 3   # ✅ Max optimization for mobile
lto       = true # ✅ Link-time optimization — smaller binary
panic     = "abort" # ✅ No unwinding on mobile — saves ~200KB
strip     = true # ✅ Strip debug symbols
```

Consider adding `codegen-units = 1` for maximum LTO effectiveness (at the cost of
longer compile times).

---

## 3. C FFI Layer — Best Practices

### 3.1 Current Pattern (Correct)

The `ffi.rs` follows the canonical pattern for exposing Rust to C:

```rust
#[no_mangle]
pub unsafe extern "C" fn mesh_init(
    path_ptr: *const u8,
    path_len: usize,
) -> bool { ... }
```

**Best practices observed:**
- All functions are `#[no_mangle]` + `extern "C"` → C-ABI compatible
- Pointer+length pairs (not null-terminated strings) → safer, more explicit
- Null checks on every pointer parameter → defense in depth
- Buffer-based output (`buf + buf_len + out_len` triple) → no heap allocation across FFI
- Returns `bool` for status → simple, no complex error types across FFI boundary

### 3.2 C Header (`ReticulumMobile.h`)

The header correctly duplicates the FFI signatures with C types. **Best practice**:

```c
#pragma once
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif
// ... all function declarations ...
#ifdef __cplusplus
}
#endif
```

### 3.3 Known Issue: Header Inconsistency

⚠️ **Bug found**: The peer discovery functions (`mesh_peer_count`, `mesh_get_peer`,
`mesh_clear_peers`) are declared **outside** the `extern "C"` block in the header:

```c
#ifdef __cplusplus
}
#endif

// ← These are NOT inside the extern "C" block!
uint32_t mesh_peer_count(void);
bool mesh_get_peer(...);
void mesh_clear_peers(void);
```

**Fix**: Move all peer discovery declarations inside the `extern "C"` block.

### 3.4 Recommendation: Use `cbindgen`

Instead of manually maintaining the `.h` file, use
[cbindgen](https://github.com/eyre-rs/cbindgen) to auto-generate it:

```toml
# cbindgen.toml
language = "C"
header = "#pragma once"
include_guard = "RETICULUM_MOBILE_H"
```

```bash
cbindgen --config cbindgen.toml --crate reticulum_mobile --output ReticulumMobile.h
```

---

## 4. JNI Bridge (Android) — Best Practices

### 4.1 Current Pattern (Correct)

The `jni_bridge.rs` mirrors every C FFI function with the JNI naming convention:

```rust
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshInit(
    mut env: JNIEnv,
    _class:  JClass,
    path:    JString,
) -> jboolean { ... }
```

**Key conventions followed:**
- Uses `extern "system"` (not `extern "C"`) — required for JNI on Android
- Function names match `Java_{package}_{class}_{method}` exactly
- Delegates to `ffi::mesh_*()` functions → single source of truth for logic
- Handles JNI type conversions (`JString` → `String`, `JByteArray` → `Vec<u8>`,
  `Vec<u8>` → `jbyteArray`)
- Returns `std::ptr::null_mut()` for "no data" cases

### 4.2 Recommendations

```
CURRENT                                RECOMMENDED
─────────────────────────────────────  ─────────────────────────────────────
meshPopTx allocates 4096-byte buf      Use a thread-local or static buf to
every call                             avoid repeated allocation in hot path.

No JNI exception handling              Wrap all operations in env.exception_
                                       check() to prevent JNI crashes.

Peer hash + app_data require 2         Add a single JNI function that returns
separate JNI calls (meshGetPeerHash    a Map<String, Object> for both fields
+ meshGetPeerAppData) — each calls     in one call.
mesh_get_peer() internally

String conversion uses env.get_string  Consider caching env references for
for every call                         repeated calls in hot loops.
```

---

## 5. iOS Native Module (Swift) — Best Practices

### 5.1 Expo Module Definition Pattern

The `ReticulumModule.swift` correctly uses the Expo Modules API:

```swift
public class ReticulumModule: Module {
    public func definition() -> ModuleDefinition {
        Name("ReticulumModule")
        Events("onPacketReceived", "onTxReceived", "onMeshStateChanged", "onOutgoingPacket")
        AsyncFunction("init") { ... }
        Function("stop") { ... }
    }
}
```

**Best practices observed:**
- `AsyncFunction` for operations that interact with Rust runtime (init, start, sendTx)
- `Function` for synchronous getters (isRunning, addInterface, peerCount)
- Promise-based reject/resolve with descriptive error codes
- Timer-based polling via `DispatchSourceTimer` on a dedicated utility queue

### 5.2 Poll Loop Architecture

```swift
// ✅ RX poll: drains decoded inbound packets → JS events
private func startPollLoop() {
    let t = DispatchSource.makeTimerSource(queue: pollQueue)
    t.schedule(deadline: .now(), repeating: .milliseconds(80))  // 12.5 Hz
    t.setEventHandler { [weak self] in self?.pollOnce() }
    t.resume()
    pollTimer = t
}

// ✅ TX drain: pops outgoing packets → JS onOutgoingPacket events
private func startTxDrainLoop() {
    let t = DispatchSource.makeTimerSource(queue: pollQueue)
    t.schedule(deadline: .now(), repeating: .milliseconds(20))  // 50 Hz
    t.resume()
    txTimer = t
}
```

**Design rationale:**
- RX poll at 80ms: Good balance between latency and CPU. Mesh data is not
  sub-frame latency-sensitive.
- TX drain at 20ms: Faster because outgoing radio writes must not queue up and
  cause packet loss.
- Both timers share `pollQueue` (`QOS: .utility`) — correct priority for background I/O.

### 5.3 Known Issue: Swift Scope / Brace Error

⚠️ **Bug found**: The `peerCount()`, `peers()` and `clearPeers()` functions are defined
**outside** the `definition() -> ModuleDefinition` return scope but still inside the
class body. Lines 127–151 appear to be at the wrong nesting level:

```swift
    }  // ← This closes definition()

    // ← These are class methods, NOT part of the module definition!
    Function("peerCount") -> Int32 { ... }    // ← COMPILE ERROR
    Function("peers") -> [[String: Any]] { ... }
    Function("clearPeers") { ... }
```

**Fix**: Move the peer discovery `Function(...)` declarations _inside_ the
`definition()` method, before the closing `}`.

### 5.4 C Bridge Calling Convention

The Swift code correctly converts Swift strings to C pointers:

```swift
identityPath.withCString { cstr in
    mesh_init(UnsafePointer<UInt8>(OpaquePointer(cstr)), identityPath.utf8.count)
}
```

**Recommendation**: Document that this is a zero-copy bridge — no allocation happens
because `withCString` provides a temporary stack pointer.

---

## 6. Android Native Module (Kotlin) — Best Practices

### 6.1 Module Definition Pattern

```kotlin
class ReticulumModule : Module() {
    override fun definition() = ModuleDefinition {
        Name("ReticulumModule")
        Events(...)
        AsyncFunction("init") { identityPath: String, promise: Promise -> ... }
        Function("isRunning") { meshIsRunning() }
        OnDestroy { stopPollLoop(); meshStop(); scope.cancel() }
    }
}
```

**Best practices observed:**
- `OnDestroy` hook for cleanup — critical on Android where activity lifecycle is complex
- `CoroutineScope(Dispatchers.IO + SupervisorJob())` for the poll loop
- `System.loadLibrary("reticulum_mobile")` in companion `init` block — loads `.so`
  eagerly on class load

### 6.2 JNI Loading Pattern

```kotlin
companion object {
    init {
        try {
            System.loadLibrary("reticulum_mobile")
            Log.i(TAG, "libreticulum_mobile.so loaded")
        } catch (e: UnsatisfiedLinkError) {
            Log.e(TAG, "Failed to load native lib: ${e.message}")
        }
    }
}
```

**Recommendations:**
- Consider adding a `private var nativeLoaded = false` flag and checking it before
  every JNI call to prevent crashes when the `.so` fails to load.
- Handle the `UnsatisfiedLinkError` more gracefully — currently the module will crash
  on the first JNI call if loading failed.

### 6.3 Poll Loop

The unified `drainRx()` + `drainTx()` approach in a single coroutine at 20ms is cleaner
than the iOS dual-timer approach. Both are valid.

---

## 7. TypeScript API Layer — Best Practices

### 7.1 Module Binding Pattern

```typescript
import { NativeModulesProxy, EventEmitter, Subscription } from 'expo-modules-core';
const ReticulumNative = NativeModulesProxy.ReticulumModule;
const emitter = new EventEmitter(ReticulumNative);
```

**This is the standard Expo Modules pattern.** The `NativeModulesProxy` automatically
resolves to the correct platform module (Swift or Kotlin).

### 7.2 Type-Safe Event System

```typescript
export interface PacketReceivedEvent { iface: string; data: number[]; }
export interface TxReceivedEvent     { data: number[]; }
export interface MeshStateEvent      { running: boolean; }
export interface OutgoingPacketEvent  { iface: string; data: number[]; }
```

**Best practice**: All events have typed interfaces. The JS layer converts between
`number[]` ↔ `Uint8Array` at the hook level.

### 7.3 The `useMesh()` Hook

The React hook pattern is excellent for anon0mesh integration:

```typescript
export function useMesh(options: UseMeshOptions = {}): UseMeshReturn {
    // Bootstrap: init + addInterface on mount
    // Subscribe to all events with stable refs
    // Expose start/stop/sendTx/sendTo/pushRx
    // Cleanup on unmount
}
```

**Key patterns:**
- `useRef` for callback stability (prevents re-subscription on every render)
- `useCallback` for all exposed actions (stable references for child components)
- Identity directory auto-creation via `expo-file-system`
- Packet history capped at 200 entries (prevents memory leaks)
- Cleanup `useEffect` that calls `Reticulum.stop()` on unmount

### 7.4 Recommendations

```
CURRENT                                RECOMMENDED
─────────────────────────────────────  ─────────────────────────────────────
data: number[] in events               Consider using Uint8Array/ArrayBuffer
                                       directly if Expo Modules supports it —
                                       avoids number[] ↔ Uint8Array conversion.

pushRx converts Uint8Array to          This copy could be avoided with direct
Array.from(data) → number[]            TypedArray support in newer Expo SDK.

No reconnect/retry logic               Add exponential backoff for start()
                                       failures.

Peer list requires manual              Add a periodic auto-refresh option
refreshPeers() calls                   (e.g. every 30s) in useMesh options.

No TypeScript enum for dest_tag        Export constants: TAG_MESSAGE = 0x00,
                                       TAG_SOLANA_TX = 0x01, etc.
```

---

## 8. Build System & Toolchain — Best Practices

### 8.1 iOS Build Pipeline

```
┌─────────────────────────┐
│  build_rust_ios.sh       │
│  ├─ cargo build --target │
│  │  aarch64-apple-ios    │ → device
│  │  x86_64-apple-ios     │ → sim (intel)
│  │  aarch64-apple-ios-sim│ → sim (apple silicon)
│  ├─ lipo -create         │ → fat sim lib
│  └─ xcodebuild           │
│     -create-xcframework  │ → ReticulumMobile.xcframework
└─────────────────────────┘
         ↓
┌─────────────────────────┐
│  ExpoReticulum.podspec   │
│  vendored_frameworks =   │
│  'ios/Frameworks/        │
│   ReticulumMobile.       │
│   xcframework'           │
└─────────────────────────┘
```

**Best practices observed:**
- XCFramework bundles both device and sim slices — Xcode auto-selects
- `prepare_command` in podspec triggers Rust build on `pod install`
- Xcode build phase script handles incremental rebuilds

**Recommendations:**
- Add `CARGO_TARGET_DIR` override to avoid rebuilding in the source tree
- Add `--locked` flag to `cargo build` for reproducible CI builds
- Consider using `cargo-xwin` or `cargo-zigbuild` for deterministic cross-compilation

### 8.2 Android Build Pipeline

```
┌──────────────────────────────┐
│  build.gradle (buildRust)     │
│  cargo ndk                    │
│  --target arm64-v8a           │
│  --target armeabi-v7a         │
│  --output-dir jniLibs/        │
│  -- build --release           │
└──────────────────────────────┘
         ↓
┌──────────────────────────────┐
│  jniLibs/                     │
│  ├─ arm64-v8a/                │
│  │  └─ libreticulum_mobile.so │
│  └─ armeabi-v7a/              │
│     └─ libreticulum_mobile.so │
└──────────────────────────────┘
```

**Best practices observed:**
- `cargo-ndk` is the standard tool for Android Rust cross-compilation
- Gradle `buildRust` task auto-runs before `mergeJniLibFolders`
- Input/output tracking for incremental builds (`inputs.dir`, `outputs.dir`)
- ABI filter `arm64-v8a` + `armeabi-v7a` covers 99%+ of Android devices

**Recommendations:**
- Add `x86_64` target for Android emulator support during development
- Set `ANDROID_NDK_HOME` check with a clear error message in the Gradle task
- Consider pre-building `.so` files in CI and checking them into the repo for
  developers without NDK installed

### 8.3 Required Toolchain

| Tool | Version | Installation |
|---|---|---|
| Rust (stable) | latest | `rustup install stable` |
| `cargo-ndk` | latest | `cargo install cargo-ndk` |
| Android NDK | r25c+ | Android Studio SDK Manager |
| Xcode | 15+ | Mac App Store |
| `protoc` | 3.x | `brew install protobuf` / `apt install protobuf-compiler` |
| Node.js | 20+ | nvm / fnm |
| pnpm | 8+ | `npm install -g pnpm` |

### 8.4 Rust Target Matrix

```bash
# Required targets (all platforms)
rustup target add \
  aarch64-apple-ios \        # iOS device
  aarch64-apple-ios-sim \    # iOS simulator (Apple Silicon)
  x86_64-apple-ios \         # iOS simulator (Intel)
  aarch64-linux-android \    # Android device (arm64)
  armv7-linux-androideabi    # Android device (arm32)

# Optional (recommended for dev)
rustup target add \
  x86_64-linux-android       # Android emulator (Intel)
```

---

## 9. Integration into anon0mesh — Best Practices

### 9.1 Module Installation

In the anon0mesh app:

```json
// package.json — link as a local dependency
{
  "dependencies": {
    "expo-reticulum": "file:../reticulum-rn/expo-module"
  }
}
```

Or via workspace symlink / git submodule.

### 9.2 Podfile (iOS)

```ruby
pod 'ExpoReticulum', :path => '../reticulum-rn/expo-module'
```

### 9.3 settings.gradle (Android)

```groovy
include ':expo-reticulum'
project(':expo-reticulum').projectDir = new File('../reticulum-rn/expo-module/android')
```

### 9.4 BLE Integration Pattern

The critical integration point is bridging between the existing anon0mesh BLE layer
and the Reticulum interface system:

```typescript
// In your BLE manager / adapter:
import { useMesh } from 'expo-reticulum';

const mesh = useMesh({
  interfaces: ['ble'],
  onOutgoing: ({ iface, data }) => {
    // Rust has a packet ready to send over BLE
    if (iface === 'ble') {
      bleAdapter.writeCharacteristic(MESH_CHAR_UUID, new Uint8Array(data));
    }
  },
});

// When BLE receives data from a peer:
bleAdapter.onCharacteristicChanged(MESH_CHAR_UUID, (data: Uint8Array) => {
  mesh.pushRx('ble', data);
});
```

### 9.5 Solana Transaction Relay

```typescript
const mesh = useMesh({
  interfaces: ['ble'],
  onTx: (txBytes) => {
    // A Solana durable-nonce tx arrived via the mesh GROUP relay
    offlineTxQueue.enqueue(txBytes);
  },
});

// When user submits an offline transaction:
async function submitOfflineTx(transaction: Transaction) {
  const serialized = transaction.serialize();
  await mesh.sendTx(new Uint8Array(serialized));
}
```

### 9.6 Expo Dev Client Requirement

⚠️ **Critical**: This module contains native code. It **cannot** run in Expo Go.
You must use a **development build**:

```bash
npx expo prebuild
npx expo run:ios    # or run:android
```

---

## 10. Issues & Improvements Found in Current Codebase

### 10.1 Critical Bugs

| # | File | Issue | Severity |
|---|---|---|---|
| 1 | `ios/ReticulumMobile.h` | Peer discovery functions are outside `extern "C"` block — C++ name mangling will break linking | 🔴 High |
| 2 | `ios/ReticulumModule.swift` | `peerCount()`, `peers()`, `clearPeers()` are outside the `definition()` method scope — **will not compile** | 🔴 High |
| 3 | `rust-core/src/node.rs` L370 | `Arc::get_mut()` will panic if any clone exists — fragile startup sequence | 🟡 Medium |
| 4 | `rust-core/src/node.rs` L51 | `cargo check` output piped through `tail -5` hides warnings/errors | 🟢 Low |

### 10.2 Design Improvements

| # | Area | Current | Recommended |
|---|---|---|---|
| 1 | Error types | All FFI returns `bool` | Add an error code enum (u32) for diagnostics |
| 2 | Logging | Uses `log` crate with `env_logger`/`android_log` | Add `oslog` crate for iOS for native unified logging |
| 3 | Identity | Loaded up to 3 times during startup | Cache in a `OnceCell` or `OnceLock` |
| 4 | Memory | `MeshPacket` history uncapped in native | Already capped to 200 in useMesh.ts — good |
| 5 | Thread safety | `std::sync::Mutex` inside async context | Use `parking_lot::Mutex` (no poisoning) or `tokio::sync::Mutex` |
| 6 | Binary size | LTO enabled but `codegen-units` unset | Add `codegen-units = 1` for max LTO |
| 7 | Testing | No test files present | Add Rust unit tests + integration tests |
| 8 | CI | No CI configuration | Add GitHub Actions for cross-compile + typecheck |

### 10.3 API Completeness

| Reticulum Feature | Exposed to JS? | Notes |
|---|---|---|
| Identity creation/persistence | ✅ | Via `init(identityPath)` |
| SINGLE destination (DM) | ✅ | `sendTo()` + `onPacketReceived` |
| GROUP destination (broadcast) | ✅ | `sendTx()` + `onTxReceived` |
| Interface registration | ✅ | `addInterface("ble" \| "lora")` |
| Radio I/O (push/pop) | ✅ | `pushRx()` + `onOutgoingPacket` |
| Peer discovery | ✅ | `peers()` + `refreshPeers()` |
| Announce (periodic) | ✅ | Auto-announces every 300s |
| Link establishment | ❌ | Not yet implemented |
| Resource transfer | ❌ | Not yet implemented |
| Path discovery | ❌ | Event exists but no Rust impl |
| Custom app_data in announce | ❌ | Hardcoded to `None` |
| IFAC (Interface Authentication) | ❌ | Uses `IfacFlag::Open` only |

---

## 11. Recommended Migration Checklist

When porting this into the anon0mesh mobile app:

### Phase 1: Fix Critical Bugs
- [ ] Fix `ReticulumMobile.h` — move peer functions inside `extern "C"` block
- [ ] Fix `ReticulumModule.swift` — move peer functions inside `definition()`
- [ ] Test compilation on both iOS and Android

### Phase 2: Build System Integration
- [ ] Add the `rust-core/` as a git submodule or workspace link
- [ ] Verify `cargo check` passes for all targets
- [ ] Run `build_rust_ios.sh` and verify `.xcframework` output
- [ ] Run `cargo ndk` and verify `.so` output for `arm64-v8a`
- [ ] Add `x86_64-linux-android` target for emulator dev
- [ ] Run `pod install` with the podspec
- [ ] Verify Android Gradle `buildRust` task succeeds

### Phase 3: TypeScript Integration
- [ ] Install `expo-reticulum` as a local dependency
- [ ] Wire `useMesh()` into the existing BLE adapter
- [ ] Connect `onOutgoing` → BLE write characteristic
- [ ] Connect BLE read → `pushRx()`
- [ ] Test peer discovery between two devices
- [ ] Test message sending (SINGLE destination)
- [ ] Test Solana TX relay (GROUP destination)

### Phase 4: Production Hardening
- [ ] Add error codes to FFI layer (replace `bool` returns)
- [ ] Add Rust unit tests (`cargo test`)
- [ ] Add CI pipeline (GitHub Actions)
- [ ] Add `cbindgen` for auto-generated headers
- [ ] Implement Link establishment (upstream Reticulum feature)
- [ ] Set custom `app_data` in announce (display name)
- [ ] Add IFAC support for interface authentication
- [ ] Profile binary size and optimize (currently ~2-4MB per arch)
- [ ] Add crash reporting integration (Sentry/Bugsnag native)
- [ ] Document the full setup for new developers

---

## Appendix: File Map

```
reticulum-rn/
├── README.md                          # Project overview + architecture diagram
├── setup.sh                           # One-shot bootstrap script
├── rust-core/
│   ├── Cargo.toml                     # Crate config: reticulum 0.1.0, tokio, jni
│   ├── Cargo.lock                     # Locked dependencies
│   ├── build.rs                       # Passthrough (reticulum handles proto)
│   └── src/
│       ├── lib.rs                     # Module declarations (node, ffi, jni_bridge)
│       ├── node.rs                    # MeshNode, BLEDriver, LoRaDriver, PeerTable
│       ├── ffi.rs                     # C FFI surface (consumed by iOS Swift)
│       └── jni_bridge.rs             # JNI bridge (consumed by Android Kotlin)
├── expo-module/
│   ├── package.json                   # npm package: expo-reticulum
│   ├── expo-module.config.json        # Expo auto-linking config
│   ├── ExpoReticulum.podspec          # CocoaPods spec (iOS)
│   ├── tsconfig.json                  # TypeScript config
│   ├── src/
│   │   ├── index.ts                   # Public API: init, start, stop, sendTx, etc.
│   │   ├── useMesh.ts                 # React hook: useMesh()
│   │   └── MeshScreen.tsx             # Example screen component
│   ├── ios/
│   │   ├── ReticulumModule.swift      # Expo Module (Swift → C FFI)
│   │   ├── ReticulumMobile.h          # C header for Rust FFI
│   │   ├── ExpoReticulum-Bridging-Header.h
│   │   ├── build_rust_ios.sh          # Rust → xcframework build script
│   │   └── Frameworks/               # Built xcframework output
│   └── android/
│       ├── build.gradle               # Gradle config + buildRust task
│       └── src/main/
│           ├── AndroidManifest.xml
│           ├── jniLibs/               # Built .so files
│           │   ├── arm64-v8a/libreticulum_mobile.so
│           │   └── armeabi-v7a/libreticulum_mobile.so
│           └── kotlin/expo/modules/reticulum/
│               ├── ReticulumModule.kt  # Expo Module (Kotlin → JNI)
│               └── ReticulumPackage.kt # Expo package registration
```
