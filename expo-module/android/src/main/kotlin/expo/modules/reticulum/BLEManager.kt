package expo.modules.reticulum

import android.annotation.SuppressLint
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.Context
import android.os.ParcelUuid
import android.util.Log
import java.util.*
import java.util.concurrent.ConcurrentHashMap

private const val TAG = "ReticulumBLE"

/**
 * Manages BLE Transport for Reticulum.
 * Supports:
 * 1. Device-to-Device Mesh (GATT Server + Scanner/Client)
 * 2. RNode over BLE (Central connecting to RNode Peripheral)
 */
@SuppressLint("MissingPermission")
class BLEManager(private val context: Context, private val onDataReceived: (String, ByteArray) -> Unit) {

    private val bluetoothManager = context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    private val adapter = bluetoothManager.adapter
    private val scanner = adapter?.bluetoothLeScanner
    
    // UUIDs
    private val MESH_SERVICE_UUID = UUID.fromString("e9e00001-bbd4-42b1-9494-0f7256199342")
    private val MESH_CHAR_RX_UUID  = UUID.fromString("e9e00002-bbd4-42b1-9494-0f7256199342") // Client Writes to this
    private val MESH_CHAR_TX_UUID  = UUID.fromString("e9e00003-bbd4-42b1-9494-0f7256199342") // Client Listens to this
    
    private val RNODE_SERVICE_UUID = UUID.fromString("0000181b-0000-1000-8000-00805f9b34fb")
    private val RNODE_CHAR_DATA_UUID = UUID.fromString("00000002-0000-1000-8000-00805f9b34fb")

    private var gattServer: BluetoothGattServer? = null
    private val connectedDevices = ConcurrentHashMap<String, BluetoothGatt>()
    private val serverDevices = ConcurrentHashMap<String, BluetoothDevice>()

    // Advertising
    private val advertiser = adapter?.bluetoothLeAdvertiser
    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
            Log.i(TAG, "BLE Advertising started")
        }
    }

    // Scanning
    private val scanCallback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            val device = result.device
            if (connectedDevices.containsKey(device.address)) return
            
            // Connect to discovered Reticulum or RNode devices
            Log.i(TAG, "Discovered BLE device: ${device.address}")
            device.connectGatt(context, false, gattClientCallback)
        }
    }

    private val gattClientCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            if (newState == BluetoothProfile.STATE_CONNECTED) {
                Log.i(TAG, "Connected to GATT: ${gatt.device.address}")
                connectedDevices[gatt.device.address] = gatt
                gatt.discoverServices()
            } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                connectedDevices.remove(gatt.device.address)
                Log.i(TAG, "Disconnected from GATT: ${gatt.device.address}")
            }
        }

        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            if (status == BluetoothGatt.GATT_SUCCESS) {
                // Check if it's an RNode
                val rnodeService = gatt.getService(RNODE_SERVICE_UUID)
                if (rnodeService != null) {
                    val char = rnodeService.getCharacteristic(RNODE_CHAR_DATA_UUID)
                    if (char != null) {
                        Log.i(TAG, "Identified RNode at ${gatt.device.address}")
                        gatt.setCharacteristicNotification(char, true)
                        val desc = char.getDescriptor(UUID.fromString("00002902-0000-1000-8000-00805f9b34fb"))
                        desc.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                        gatt.writeDescriptor(desc)
                    }
                }
                
                // Check if it's another Reticulum node
                val meshService = gatt.getService(MESH_SERVICE_UUID)
                if (meshService != null) {
                    val char = meshService.getCharacteristic(MESH_CHAR_TX_UUID)
                    if (char != null) {
                        Log.i(TAG, "Identified Reticulum Peer at ${gatt.device.address}")
                        gatt.setCharacteristicNotification(char, true)
                        val desc = char.getDescriptor(UUID.fromString("00002902-0000-1000-8000-00805f9b34fb"))
                        desc.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                        gatt.writeDescriptor(desc)
                    }
                }
            }
        }

        override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            val iface = if (characteristic.service.uuid == RNODE_SERVICE_UUID) "lora" else "ble"
            onDataReceived(iface, characteristic.value)
        }
    }

    fun start() {
        if (adapter == null || !adapter.isEnabled) return
        
        startGattServer()
        startAdvertising()
        startScanning()
    }

    fun stop() {
        stopScanning()
        stopAdvertising()
        gattServer?.close()
        gattServer = null
        connectedDevices.values.forEach { it.close() }
        connectedDevices.clear()
    }

    private fun startGattServer() {
        val callback = object : BluetoothGattServerCallback() {
            override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
                if (newState == BluetoothProfile.STATE_CONNECTED) {
                    serverDevices[device.address] = device
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    serverDevices.remove(device.address)
                }
            }

            override fun onCharacteristicWriteRequest(
                device: BluetoothDevice, requestId: Int, characteristic: BluetoothGattCharacteristic,
                preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray
            ) {
                if (characteristic.uuid == MESH_CHAR_RX_UUID) {
                    onDataReceived("ble", value)
                    if (responseNeeded) gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
                }
            }
        }

        gattServer = bluetoothManager.openGattServer(context, callback)
        val service = BluetoothGattService(MESH_SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        
        val rxChar = BluetoothGattCharacteristic(MESH_CHAR_RX_UUID, 
            BluetoothGattCharacteristic.PROPERTY_WRITE or BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
            BluetoothGattCharacteristic.PERMISSION_WRITE)
            
        val txChar = BluetoothGattCharacteristic(MESH_CHAR_TX_UUID,
            BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_READ)
        txChar.addDescriptor(BluetoothGattDescriptor(UUID.fromString("00002902-0000-1000-8000-00805f9b34fb"), 
            BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE))
            
        service.addCharacteristic(rxChar)
        service.addCharacteristic(txChar)
        gattServer?.addService(service)
    }

    private fun startAdvertising() {
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
            .setConnectable(true)
            .setTimeout(0)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
            .build()
        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(MESH_SERVICE_UUID))
            .build()
        advertiser?.startAdvertising(settings, data, advertiseCallback)
    }

    private fun startScanning() {
        val filters = listOf(
            ScanFilter.Builder().setServiceUuid(ParcelUuid(MESH_SERVICE_UUID)).build(),
            ScanFilter.Builder().setServiceUuid(ParcelUuid(RNODE_SERVICE_UUID)).build()
        )
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_BALANCED)
            .build()
        scanner?.startScan(filters, settings, scanCallback)
    }

    private fun stopScanning() {
        scanner?.stopScan(scanCallback)
    }

    private fun stopAdvertising() {
        advertiser?.stopAdvertising(advertiseCallback)
    }

    fun sendData(iface: String, data: ByteArray) {
        if (iface == "ble") {
            // Send to all connected clients (as server)
            val service = gattServer?.getService(MESH_SERVICE_UUID)
            val char = service?.getCharacteristic(MESH_CHAR_TX_UUID)
            if (char != null) {
                char.value = data
                serverDevices.values.forEach { device ->
                    gattServer?.notifyCharacteristicChanged(device, char, false)
                }
            }
            
            // Send to all connected servers (as client)
            connectedDevices.values.forEach { gatt ->
                val meshService = gatt.getService(MESH_SERVICE_UUID)
                val rxChar = meshService?.getCharacteristic(MESH_CHAR_RX_UUID)
                if (rxChar != null) {
                    rxChar.value = data
                    gatt.writeCharacteristic(rxChar)
                }
            }
        } else if (iface == "lora") {
            // Send to RNodes
            connectedDevices.values.forEach { gatt ->
                val rnodeService = gatt.getService(RNODE_SERVICE_UUID)
                val dataChar = rnodeService?.getCharacteristic(RNODE_CHAR_DATA_UUID)
                if (dataChar != null) {
                    dataChar.value = data
                    gatt.writeCharacteristic(dataChar)
                }
            }
        }
    }
}
