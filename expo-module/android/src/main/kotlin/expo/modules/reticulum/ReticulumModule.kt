package expo.modules.reticulum

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.Promise
import kotlinx.coroutines.*
import android.util.Log

private const val TAG = "ReticulumModule"

class ReticulumModule : Module() {

    private val scope   = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private var polling = false

    private val ifaceNames = listOf("ble", "lora", "auto")

    override fun definition() = ModuleDefinition {

        Name("ReticulumModule")

        Events(
            "onPacketReceived",   // { iface: String, data: List<Int> } — SINGLE dest
            "onTxReceived",       // { data: List<Int> }                — GROUP tx relay
            "onMeshStateChanged", // { running: Boolean }
            "onOutgoingPacket"    // { iface: String, data: List<Int> } — bytes to transmit
        )

        // ── Lifecycle ──────────────────────────────────────────────────────

        AsyncFunction("init") { identityPath: String, promise: Promise ->
            if (meshInit(identityPath)) promise.resolve(true)
            else promise.reject("INIT_FAILED", "mesh_init returned false", null)
        }

        AsyncFunction("start") { promise: Promise ->
            if (meshStart()) {
                startPollLoop()
                sendEvent("onMeshStateChanged", mapOf("running" to true))
                promise.resolve(true)
            } else {
                promise.reject("START_FAILED", "Node not initialised or already running", null)
            }
        }

        Function("stop") {
            stopPollLoop()
            meshStop()
            sendEvent("onMeshStateChanged", mapOf("running" to false))
        }

        Function("isRunning") { meshIsRunning() }

        // ── Interface registration ─────────────────────────────────────────

        Function("addInterface") { name: String, arg: String?, mode: String? ->
            meshAddInterface(name, arg ?: "", mode ?: "full")
        }

        // ── Native radio I/O ───────────────────────────────────────────────

        Function("pushRx") { ifaceName: String, data: ByteArray ->
            meshPushRx(ifaceName, data)
        }

        // ── Sending ────────────────────────────────────────────────────────

        AsyncFunction("sendTx") { txBytes: ByteArray, promise: Promise ->
            if (meshSendTx(txBytes)) promise.resolve(true)
            else promise.reject("SEND_TX_FAILED", "Node not running or no interfaces", null)
        }

        AsyncFunction("sendTo") { destHex: String, payload: ByteArray, promise: Promise ->
            if (destHex.length != 32) {
                promise.reject("INVALID_HASH", "dest hash must be 32 hex chars", null)
                return@AsyncFunction
            }
            promise.resolve(meshSendTo(destHex, payload))
        }

        // ── Identity ───────────────────────────────────────────────────────

        AsyncFunction("localHash") { promise: Promise ->
            val h = meshLocalHash()
            if (h != null) promise.resolve(h)
            else promise.reject("NOT_INIT", "Node not initialised", null)
        }

        AsyncFunction("txGroupHash") { promise: Promise ->
            val h = meshTxGroupHash()
            if (h != null) promise.resolve(h)
            else promise.reject("NOT_INIT", "Node not initialised", null)
        }


        // ── Peer discovery ─────────────────────────────────────────────────

        Function("peerCount") {
            meshPeerCount()
        }

        /// Returns a list of peer maps: [{ hash: String, appData: List<Int> }]
        Function("peers") -> List<Map<String, Any>> {
            val count = meshPeerCount()
            (0 until count).mapNotNull { i ->
                val hash    = meshGetPeerHash(i) ?: return@mapNotNull null
                val appData = meshGetPeerAppData(i)?.map { it.toInt() and 0xFF } ?: emptyList()
                mapOf("hash" to hash, "appData" to appData)
            }
        }

        Function("clearPeers") {
            meshClearPeers()
        }

        OnDestroy {
            stopPollLoop()
            meshStop()
            scope.cancel()
        }
    }

    // ── Poll + TX-drain loop ──────────────────────────────────────────────

    private fun startPollLoop() {
        if (polling) return
        polling = true
        scope.launch {
            while (polling) {
                drainRx()
                drainTx()
                delay(20)
            }
        }
    }

    private fun stopPollLoop() { polling = false }

    /** Drain decoded inbound packets and emit JS events. */
    private fun drainRx() {
        while (true) {
            val raw = meshPoll() ?: break
            if (raw.isEmpty()) break
            val destTag = raw[0].toInt() and 0xFF
            val payload = raw.drop(1).map { it.toInt() and 0xFF }
            if (destTag == 0x01) {
                sendEvent("onTxReceived", mapOf("data" to payload))
            } else {
                sendEvent("onPacketReceived", mapOf("iface" to "reticulum", "data" to payload))
            }
        }
    }

    /** Drain outgoing packets from each interface and emit onOutgoingPacket. */
    private fun drainTx() {
        for (iface in ifaceNames) {
            while (true) {
                val bytes = meshPopTx(iface) ?: break
                sendEvent("onOutgoingPacket", mapOf(
                    "iface" to iface,
                    "data"  to bytes.map { it.toInt() and 0xFF },
                ))
            }
        }
    }

    // ── JNI declarations ─────────────────────────────────────────────────

    private external fun meshInit(identityPath: String): Boolean
    private external fun meshStart(): Boolean
    private external fun meshStop()
    private external fun meshIsRunning(): Boolean

    /** Returns interface index ≥ 0, or -1 on failure. */
    private external fun meshAddInterface(name: String, arg: String, mode: String): Int

    private external fun meshPushRx(ifaceName: String, data: ByteArray): Boolean
    /** Returns next outgoing packet for the named interface, or null. */
    private external fun meshPopTx(ifaceName: String): ByteArray?

    private external fun meshSendTx(txBytes: ByteArray): Boolean
    private external fun meshSendTo(destHex: String, payload: ByteArray): Boolean

    /** Returns null if no inbound packet is available. */
    private external fun meshPoll(): ByteArray?

    /** Returns null if node not initialised. */
    private external fun meshLocalHash(): String?
    private external fun meshTxGroupHash(): String?

    private external fun meshPeerCount(): Int
    private external fun meshGetPeerHash(index: Int): String?
    private external fun meshGetPeerAppData(index: Int): ByteArray?
    private external fun meshClearPeers()

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
}
