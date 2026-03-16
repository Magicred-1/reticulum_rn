# reticulum-rn

Rust + Expo native module bridging [Reticulum-rs](https://github.com/BeechatNetworkSystemsLtd/Reticulum-rs)
to React Native (iOS + Android) for the [anon0mesh](https://anonme.sh) project.

## Architecture

```
┌──────────────────────────────────────────────────┐
│          React Native / Expo (TypeScript)         │
│   useMesh()  ·  index.ts  ·  MeshScreen.tsx      │
├──────────────────────────────────────────────────┤
│        Expo Modules API                           │
│   ReticulumModule.swift  ·  ReticulumModule.kt    │
├──────────────────────────────────────────────────┤
│        C FFI / JNI                                │
│   ffi.rs  ·  jni_bridge.rs                       │
├──────────────────────────────────────────────────┤
│        Rust core (reticulum crate 0.1.0)          │
│   node.rs — MeshNode, BLEDriver, LoRaDriver       │
└──────────────────────────────────────────────────┘
```

## Prerequisites

| Tool | Version | Purpose |
|---|---|---|
| Rust | stable | Core library |
| `cargo-ndk` | latest | Android `.so` builds |
| Android NDK | r25c+ | Android cross-compile |
| Xcode | 15+ | iOS `.xcframework` |
| `protoc` | 3.x | reticulum crate proto |
| Node / pnpm | 20+ / 8+ | JS layer |

Install Rust targets once:
```bash
rustup target add \
  aarch64-apple-ios \
  aarch64-apple-ios-sim \
  x86_64-apple-ios \
  aarch64-linux-android \
  armv7-linux-androideabi
```

Install cargo-ndk:
```bash
cargo install cargo-ndk
```

## iOS setup

1. Run the Rust build (or let CocoaPods run it via `prepare_command`):
   ```bash
   bash expo-module/ios/build_rust_ios.sh
   ```
2. In your host app's `Podfile`:
   ```ruby
   pod 'ExpoReticulum', :path => '../expo-reticulum'
   ```
3. Run `pod install`.

## Android setup

The Gradle task `buildRust` runs `cargo ndk` automatically before
`mergeJniLibFolders`. Set `ANDROID_NDK_HOME` or add `ndk.dir` to
`local.properties` in the Android project.

In the host app's `settings.gradle`:
```groovy
include ':expo-reticulum'
project(':expo-reticulum').projectDir = new File('../expo-reticulum/android')
```

## Usage

```ts
import { useMesh } from 'expo-reticulum';

export default function App() {
  const { running, localHash, txGroupHash, start, stop, sendTx } = useMesh({
    interfaces: ['ble', 'lora'],
    onTx: (txBytes) => {
      // Solana durable-nonce tx arrived via GROUP relay
      // → hand off to your offline tx queue
    },
    onOutgoing: ({ iface, data }) => {
      // Rust wants to transmit these bytes over a physical radio
      // → write to BLE characteristic or LoRa serial
    },
  });
  // ...
}
```

## Packet wire format

Every item emitted from the Rust rx queue is prefixed with one `dest_tag` byte:

| dest_tag | Destination | JS event |
|---|---|---|
| `0x00` | SINGLE (our node) | `onPacketReceived` |
| `0x01` | GROUP tx relay | `onTxReceived` |

## License

MIT
