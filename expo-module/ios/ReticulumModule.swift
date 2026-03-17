import ExpoModulesCore
import Foundation

public class ReticulumModule: Module {

    private let pollQueue = DispatchQueue(label: "sh.anonme.reticulum.poll", qos: .utility)
    private var pollTimer: DispatchSourceTimer?
    private let pollBufSize = 4096

    // TX-drain timer — pops outgoing packets and hands them to native BLE/LoRa layer
    private var txTimer: DispatchSourceTimer?

    public func definition() -> ModuleDefinition {
        Name("ReticulumModule")

        Events(
            "onPacketReceived",   // { iface: "reticulum", data: [UInt8] } — SINGLE dest
            "onTxReceived",       // { data: [UInt8] }                     — GROUP tx relay
            "onMeshStateChanged", // { running: Bool }
            "onOutgoingPacket"    // { iface: String, data: [UInt8] }      — bytes to transmit
        )

        // ── Lifecycle ──────────────────────────────────────────────────────

        AsyncFunction("init") { (identityPath: String, promise: Promise) in
            let ok = identityPath.withCString { cstr in
                mesh_init(UnsafePointer<UInt8>(OpaquePointer(cstr)), identityPath.utf8.count)
            }
            ok ? promise.resolve(true)
               : promise.reject("INIT_FAILED", "mesh_init returned false")
        }

        AsyncFunction("start") { (promise: Promise) in
            let ok = mesh_start()
            if ok {
                self.startPollLoop()
                self.startTxDrainLoop()
                self.sendEvent("onMeshStateChanged", ["running": true])
                promise.resolve(true)
            } else {
                promise.reject("START_FAILED", "Node not initialised or already running")
            }
        }

        Function("stop") {
            self.stopPollLoop()
            self.stopTxDrainLoop()
            mesh_stop()
            self.sendEvent("onMeshStateChanged", ["running": false])
        }

        Function("isRunning") -> Bool { mesh_is_running() }

        // ── Interface registration ─────────────────────────────────────────

        /// Returns the interface index, or -1 on failure. Must call before start().
        /// Supported names: "ble", "lora", "auto", "tcp_client", "tcp_server"
        /// arg is optional and used for TCP addresses (e.g. "1.2.3.4:4242")
        Function("addInterface") { (name: String, arg: String?, mode: String?) -> Int32 in
            name.withCString { nameCStr in
                let argStr  = arg  ?? ""
                let modeStr = mode ?? "full"
                return argStr.withCString { argCStr in
                    modeStr.withCString { modeCStr in
                        mesh_add_interface(
                            UnsafePointer<UInt8>(OpaquePointer(nameCStr)), name.utf8.count,
                            UnsafePointer<UInt8>(OpaquePointer(argCStr)),  argStr.utf8.count,
                            UnsafePointer<UInt8>(OpaquePointer(modeCStr)), modeStr.utf8.count
                        )
                    }
                }
            }
        }

        // ── Native radio I/O ───────────────────────────────────────────────

        /// Called from Swift BLE/LoRa delegate when bytes arrive from the radio.
        Function("pushRx") { (ifaceName: String, data: [UInt8]) -> Bool in
            ifaceName.withCString { nameCStr in
                data.withUnsafeBufferPointer { dataBuf in
                    mesh_push_rx(
                        UnsafePointer<UInt8>(OpaquePointer(nameCStr)), ifaceName.utf8.count,
                        dataBuf.baseAddress, data.count
                    )
                }
            }
        }

        // ── Sending ────────────────────────────────────────────────────────

        AsyncFunction("sendTx") { (txBytes: [UInt8], promise: Promise) in
            let ok = txBytes.withUnsafeBufferPointer { buf in
                mesh_send_tx(buf.baseAddress, txBytes.count)
            }
            ok ? promise.resolve(true)
               : promise.reject("SEND_TX_FAILED", "Node not running or no interfaces")
        }

        AsyncFunction("sendTo") { (destHex: String, payload: [UInt8], promise: Promise) in
            guard destHex.count == 32 else {
                promise.reject("INVALID_HASH", "dest hash must be 32 hex chars")
                return
            }
            let ok = destHex.withCString { hexCStr in
                payload.withUnsafeBufferPointer { payloadBuf in
                    mesh_send_to(
                        UnsafePointer<UInt8>(OpaquePointer(hexCStr)), destHex.utf8.count,
                        payloadBuf.baseAddress, payload.count
                    )
                }
            }
            promise.resolve(ok)
        }

        // ── Identity ───────────────────────────────────────────────────────

        AsyncFunction("localHash") { (promise: Promise) in
            var buf = [UInt8](repeating: 0, count: 33)
            if mesh_local_hash(&buf, buf.count) {
                promise.resolve(String(bytes: buf.prefix(32), encoding: .utf8) ?? "")
            } else {
                promise.reject("NOT_INIT", "Node not initialised")
            }
        }

        AsyncFunction("txGroupHash") { (promise: Promise) in
            var buf = [UInt8](repeating: 0, count: 33)
            if mesh_tx_group_hash(&buf, buf.count) {
                promise.resolve(String(bytes: buf.prefix(32), encoding: .utf8) ?? "")
            } else {
                promise.reject("NOT_INIT", "Node not initialised")
            }
        }

        // ── Peer discovery ─────────────────────────────────────────────────

        Function("peerCount") -> Int32 {
            Int32(mesh_peer_count())
        }

        /// Returns an array of peer objects: [{ hash: String, appData: [UInt8] }]
        Function("peers") -> [[String: Any]] {
            let count = Int(mesh_peer_count())
            var result: [[String: Any]] = []
            for i in 0..<count {
                var hashBuf    = [UInt8](repeating: 0, count: 33)
                var appBuf     = [UInt8](repeating: 0, count: 256)
                var appDataLen = 0
                guard mesh_get_peer(UInt32(i), &hashBuf, hashBuf.count,
                                    &appBuf, appBuf.count, &appDataLen) else { continue }
                let hash    = String(bytes: hashBuf.prefix(32), encoding: .utf8) ?? ""
                let appData = Array(appBuf.prefix(appDataLen))
                result.append(["hash": hash, "appData": appData])
            }
            return result
        }

        Function("clearPeers") {
            mesh_clear_peers()
        }

        AsyncFunction("fetchMessages") { (limit: UInt32, promise: Promise) in
            var buf    = [UInt8](repeating: 0, count: 16384)
            var outLen = 0
            let ok = mesh_fetch_messages(limit, &buf, buf.count, &outLen)
            
            if !ok && outLen > buf.count {
                // Retry with larger buffer
                buf    = [UInt8](repeating: 0, count: outLen)
                let ok2 = mesh_fetch_messages(limit, &buf, buf.count, &outLen)
                if !ok2 {
                    promise.reject("FETCH_FAILED", "Failed to fetch messages (too large?)")
                    return
                }
            } else if !ok {
                promise.reject("FETCH_FAILED", "Failed to fetch messages from DB")
                return
            }
            
            let json = String(bytes: buf.prefix(outLen), encoding: .utf8) ?? "[]"
            promise.resolve(json)
        }
    }

    // ── RX poll loop — drains decoded inbound packets → JS events ─────────

    private func startPollLoop() {
        let t = DispatchSource.makeTimerSource(queue: pollQueue)
        t.schedule(deadline: .now(), repeating: .milliseconds(80))
        t.setEventHandler { [weak self] in self?.pollOnce() }
        t.resume()
        pollTimer = t
    }

    private func stopPollLoop() { pollTimer?.cancel(); pollTimer = nil }

    private func pollOnce() {
        var buf    = [UInt8](repeating: 0, count: pollBufSize)
        var outLen = 0
        while true {
            guard mesh_poll(&buf, pollBufSize, &outLen), outLen > 0 else { break }
            // dest_tag is first byte
            let destTag = buf[0]
            let payload = Array(buf[1..<outLen])
            if destTag == 0x01 {
                sendEvent("onTxReceived", ["data": payload])
            } else {
                sendEvent("onPacketReceived", ["iface": "reticulum", "data": payload])
            }
            outLen = 0
        }
    }

    // ── TX drain loop — pops outgoing bytes and fires onOutgoingPacket ────
    // The React Native BLE/LoRa layer listens to onOutgoingPacket and
    // writes the bytes to the physical radio characteristic.
    // NOTE: "auto" interface handles its own UDP I/O in Rust — its queue
    // will always be empty, so draining it is a harmless no-op.

    private func startTxDrainLoop() {
        let t = DispatchSource.makeTimerSource(queue: pollQueue)
        t.schedule(deadline: .now(), repeating: .milliseconds(20))
        t.setEventHandler { [weak self] in self?.drainTx() }
        t.resume()
        txTimer = t
    }

    private func stopTxDrainLoop() { txTimer?.cancel(); txTimer = nil }

    private let ifaceNames = ["ble", "lora", "auto"]

    private func drainTx() {
        for iface in ifaceNames {
            var buf    = [UInt8](repeating: 0, count: pollBufSize)
            var outLen = 0
            while true {
                let ok = iface.withCString { nameCStr in
                    mesh_pop_tx(
                        UnsafePointer<UInt8>(OpaquePointer(nameCStr)), iface.utf8.count,
                        &buf, pollBufSize, &outLen
                    )
                }
                guard ok, outLen > 0 else { break }
                let packet = Array(buf.prefix(outLen))
                sendEvent("onOutgoingPacket", ["iface": iface, "data": packet])
                outLen = 0
            }
        }
    }
}
