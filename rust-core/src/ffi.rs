//! C FFI — consumed directly by Swift via the bridging header.
//! All functions are safe to call from any thread.

use std::ptr;
use lazy_static::lazy_static;
use std::sync::Mutex;
use crate::node::MeshNode;

lazy_static! {
    static ref NODE: Mutex<Option<MeshNode>> = Mutex::new(None);
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/// Initialise the node with a path to persistent identity storage.
/// Returns false if already initialised or path is invalid.
#[no_mangle]
pub unsafe extern "C" fn mesh_init(
    path_ptr: *const u8,
    path_len: usize,
) -> bool {
    if path_ptr.is_null() || path_len == 0 { return false; }
    let slice = unsafe { std::slice::from_raw_parts(path_ptr, path_len) };
    let path_str = match std::str::from_utf8(slice) {
        Ok(s)  => s,
        Err(_) => return false,
    };
    let mut guard = NODE.lock().unwrap();
    if guard.is_some() { return false; }
    *guard = Some(MeshNode::new(std::path::Path::new(path_str)));
    true
}

#[no_mangle]
pub unsafe extern "C" fn mesh_start() -> bool {
    let mut guard = NODE.lock().unwrap();
    match guard.as_mut() {
        Some(n) => n.start(),
        None    => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mesh_stop() {
    if let Some(n) = NODE.lock().unwrap().as_ref() { n.stop(); }
}

#[no_mangle]
pub unsafe extern "C" fn mesh_is_running() -> bool {
    NODE.lock().unwrap().as_ref().map(|n| n.is_running()).unwrap_or(false)
}

// ── Interface registration ────────────────────────────────────────────────────

/// Register a named interface ("ble" | "lora"). Must be called before mesh_start().
/// Returns the interface index (≥ 0) on success, or -1 on failure.
/// The index is used with mesh_push_rx / mesh_pop_tx for named-iface I/O.
#[no_mangle]
pub unsafe extern "C" fn mesh_add_interface(
    name_ptr: *const u8, name_len: usize,
    arg_ptr:  *const u8, arg_len:  usize,
    mode_ptr: *const u8, mode_len: usize,
) -> i32 {
    if name_ptr.is_null() || name_len == 0 { return -1; }
    let slice = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
    let name  = match std::str::from_utf8(slice) {
        Ok(s)  => s,
        Err(_) => return -1,
    };
    
    let arg = if !arg_ptr.is_null() && arg_len > 0 {
        let slice = unsafe { std::slice::from_raw_parts(arg_ptr, arg_len) };
        std::str::from_utf8(slice).ok().map(|s| s.to_string())
    } else {
        None
    };
    let mode = if !mode_ptr.is_null() && mode_len > 0 {
        let slice = unsafe { std::slice::from_raw_parts(mode_ptr, mode_len) };
        std::str::from_utf8(slice).unwrap_or("full")
    } else {
        "full"
    };

    let static_mode = crate::node::InterfaceMode::from_str(mode);

    // We need a &'static str — only accept known names.
    let static_name: &'static str = match name {
        "ble"        => "ble",
        "lora"       => "lora",
        "auto"       => "auto",
        "tcp_client" => "tcp_client",
        "tcp_server" => "tcp_server",
        _            => return -1,
    };
    let mut guard = NODE.lock().unwrap();
    match guard.as_mut() {
        Some(n) => n.add_interface(static_name, arg, static_mode) as i32,
        None    => -1,
    }
}

// ── Data path — native radio I/O ──────────────────────────────────────────────

/// Push raw bytes received from a native BLE/LoRa callback into the named interface.
/// Called by Swift/Kotlin whenever the radio layer delivers data.
#[no_mangle]
pub unsafe extern "C" fn mesh_push_rx(
    name_ptr: *const u8,
    name_len: usize,
    data_ptr: *const u8,
    data_len: usize,
) -> bool {
    if name_ptr.is_null() || data_ptr.is_null() || data_len == 0 { return false; }
    let name_slice = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
    let name = match std::str::from_utf8(name_slice) { Ok(s) => s, Err(_) => return false };
    let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) }.to_vec();
    NODE.lock().unwrap().as_ref().map(|n| n.push_rx(name, data)).unwrap_or(false)
}

/// Pop one outgoing packet for a named interface.
/// Called by Swift/Kotlin to get bytes to transmit over BLE/LoRa.
/// Writes bytes into `buf`, writes length into `out_len`.
/// Returns true with out_len=0 if nothing to transmit.
/// Returns false on error (null, buffer too small, unknown interface).
#[no_mangle]
pub unsafe extern "C" fn mesh_pop_tx(
    name_ptr: *const u8,
    name_len: usize,
    buf:      *mut u8,
    buf_len:  usize,
    out_len:  *mut usize,
) -> bool {
    if name_ptr.is_null() || buf.is_null() || out_len.is_null() { return false; }
    let name_slice = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
    let name = match std::str::from_utf8(name_slice) { Ok(s) => s, Err(_) => return false };

    let packet = NODE.lock().unwrap().as_ref().and_then(|n| n.pop_tx(name));
    match packet {
        None => { unsafe { *out_len = 0; } true }
        Some(bytes) if bytes.len() <= buf_len => {
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
                *out_len = bytes.len();
            }
            true
        }
        Some(_) => false, // buffer too small
    }
}

// ── Sending ───────────────────────────────────────────────────────────────────

/// Send a Solana transaction through the shared GROUP relay destination.
/// No path lookup required — any reachable node will receive and propagate it.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_tx(
    tx_ptr: *const u8,
    tx_len: usize,
) -> bool {
    if tx_ptr.is_null() || tx_len == 0 { return false; }
    let tx_bytes = unsafe { std::slice::from_raw_parts(tx_ptr, tx_len) };
    NODE.lock().unwrap().as_ref().map(|n| n.send_tx(tx_bytes)).unwrap_or(false)
}

/// Send a direct message to a peer by hex destination hash (32 hex chars = 16 bytes).
/// Returns false if the node isn't running.
#[no_mangle]
pub unsafe extern "C" fn mesh_send_to(
    dest_hex_ptr: *const u8,
    dest_hex_len: usize,
    payload_ptr:  *const u8,
    payload_len:  usize,
) -> bool {
    if dest_hex_ptr.is_null() || payload_ptr.is_null() || payload_len == 0 { return false; }
    let hex_slice = unsafe { std::slice::from_raw_parts(dest_hex_ptr, dest_hex_len) };
    let dest_hex  = match std::str::from_utf8(hex_slice) { Ok(s) => s, Err(_) => return false };
    let message   = unsafe { std::slice::from_raw_parts(payload_ptr, payload_len) };
    NODE.lock().unwrap().as_ref().map(|n| n.send_message(dest_hex, message)).unwrap_or(false)
}

// ── Receive ───────────────────────────────────────────────────────────────────

/// Poll for one decoded inbound packet.
/// Wire format: [dest_tag(1)] [payload...]
///   dest_tag 0x00 = SINGLE (our node dest)  → chat / message
///   dest_tag 0x01 = GROUP  (tx relay dest)  → Solana tx
/// Returns true with out_len=0 if no packet is available.
/// Returns false on null pointer, buffer too small, or node not initialised.
#[no_mangle]
pub unsafe extern "C" fn mesh_poll(
    buf:     *mut u8,
    buf_len: usize,
    out_len: *mut usize,
) -> bool {
    if buf.is_null() || out_len.is_null() { return false; }
    let packet = NODE.lock().unwrap().as_ref().and_then(|n| n.try_recv());
    match packet {
        None => { unsafe { *out_len = 0; } true }
        Some(bytes) if bytes.len() <= buf_len => {
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
                *out_len = bytes.len();
            }
            true
        }
        Some(_) => false, // buffer too small — caller should retry with a larger buf
    }
}

// ── Identity ──────────────────────────────────────────────────────────────────

/// Write the local destination hash hex string into `buf` (caller provides 33+ bytes: 32 hex + NUL).
/// Returns false on null or node not initialised.
#[no_mangle]
pub unsafe extern "C" fn mesh_local_hash(buf: *mut u8, buf_len: usize) -> bool {
    if buf.is_null() || buf_len < 33 { return false; }
    let guard = NODE.lock().unwrap();
    if let Some(n) = guard.as_ref() {
        let hex = n.local_hash_hex();
        let bytes = hex.as_bytes();
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
            *buf.add(bytes.len()) = 0; // NUL-terminate for C/Swift convenience
        }
        true
    } else {
        false
    }
}

/// Write the GROUP tx relay hash hex string into `buf` (caller provides 33+ bytes).
/// This hash is deterministic — identical on every anon0mesh device.
#[no_mangle]
pub unsafe extern "C" fn mesh_tx_group_hash(buf: *mut u8, buf_len: usize) -> bool {
    if buf.is_null() || buf_len < 33 { return false; }
    let guard = NODE.lock().unwrap();
    if let Some(n) = guard.as_ref() {
        let hex   = n.tx_group_hash_hex();
        let bytes = hex.as_bytes();
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
            *buf.add(bytes.len()) = 0;
        }
        true
    } else {
        false
    }
}

// ── Peer discovery ────────────────────────────────────────────────────────────

/// Returns the number of reachable peers currently in the peer table.
#[no_mangle]
pub unsafe extern "C" fn mesh_peer_count() -> u32 {
    NODE.lock().unwrap()
        .as_ref()
        .map(|n| n.peer_count() as u32)
        .unwrap_or(0)
}

/// Read one peer by index (0-based, stable-sorted by hash).
///
/// Writes the 32-char hex hash into `hash_buf` (caller provides ≥ 33 bytes, NUL-terminated).
/// Writes app_data bytes into `app_data_buf` (caller provides `app_data_buf_len` bytes).
/// Writes actual app_data length into `out_app_data_len`.
///
/// Returns false if index is out of range, buffers are null, or node not initialised.
#[no_mangle]
pub unsafe extern "C" fn mesh_get_peer(
    index:            u32,
    hash_buf:         *mut u8,
    hash_buf_len:     usize,
    app_data_buf:     *mut u8,
    app_data_buf_len: usize,
    out_app_data_len: *mut usize,
) -> bool {
    if hash_buf.is_null() || out_app_data_len.is_null() { return false; }
    if hash_buf_len < 33 { return false; }

    let guard = NODE.lock().unwrap();
    let peers = match guard.as_ref() {
        Some(n) => n.peer_list(),
        None    => return false,
    };

    let peer = match peers.get(index as usize) {
        Some(p) => p,
        None    => return false,
    };

    // Write hash (32 hex chars + NUL)
    let hash_bytes = peer.hash.as_bytes();
    unsafe {
        std::ptr::copy_nonoverlapping(hash_bytes.as_ptr(), hash_buf, 32);
        *hash_buf.add(32) = 0;
    }

    // Write app_data if buffer provided and large enough
    let app_len = peer.app_data.len().min(app_data_buf_len);
    if !app_data_buf.is_null() && app_len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(peer.app_data.as_ptr(), app_data_buf, app_len); }
    }
    unsafe { *out_app_data_len = peer.app_data.len(); } // report full length even if truncated

    true
}

/// Remove all peers from the table.
#[no_mangle]
pub unsafe extern "C" fn mesh_clear_peers() {
    if let Some(n) = NODE.lock().unwrap().as_ref() { n.clear_peers(); }
}

/// Fetch historical messages from SQLite. Returns JSON array string.
/// Caller provides buffer and receives length in `out_len`.
#[no_mangle]
pub unsafe extern "C" fn mesh_fetch_messages(
    limit:   u32,
    buf:     *mut u8,
    buf_len: usize,
    out_len: *mut usize,
) -> bool {
    if buf.is_null() || out_len.is_null() { return false; }
    let guard = NODE.lock().unwrap();
    let n = match guard.as_ref() {
        Some(n) => n,
        None    => return false,
    };

    let messages = n.fetch_messages(limit as usize);
    let json     = serde_json::to_string(&messages).unwrap_or_else(|_| "[]".to_string());
    let bytes    = json.as_bytes();

    if bytes.len() > buf_len {
        unsafe { *out_len = bytes.len(); }
        return false; // buffer too small
    }

    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *out_len = bytes.len();
    }
    true
}
