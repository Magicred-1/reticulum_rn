//! Android JNI bridge.
//! Mirrors every function in ffi.rs under JNI naming:
//!   Java_expo_modules_reticulum_ReticulumModule_<camelCase>
//!
//! Kotlin declares these as `external fun` inside the companion object.

use jni::JNIEnv;
use jni::objects::{JClass, JString, JByteArray};
use jni::sys::{jboolean, jbyteArray, jint, JNI_TRUE, JNI_FALSE};
use crate::ffi;

// ── Lifecycle ─────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshInit(
    mut env: JNIEnv,
    _class:  JClass,
    path:    JString,
) -> jboolean {
    let path_str: String = match env.get_string(&path) {
        Ok(s)  => s.into(),
        Err(_) => return JNI_FALSE,
    };
    let ok = unsafe { ffi::mesh_init(path_str.as_ptr(), path_str.len()) };
    if ok { JNI_TRUE } else { JNI_FALSE }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshStart(
    _env:   JNIEnv,
    _class: JClass,
) -> jboolean {
    if unsafe { ffi::mesh_start() } { JNI_TRUE } else { JNI_FALSE }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshStop(
    _env:   JNIEnv,
    _class: JClass,
) {
    unsafe { ffi::mesh_stop() }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshIsRunning(
    _env:   JNIEnv,
    _class: JClass,
) -> jboolean {
    if unsafe { ffi::mesh_is_running() } { JNI_TRUE } else { JNI_FALSE }
}

// ── Interface registration ────────────────────────────────────────────────────

/// Returns interface index (≥ 0) or -1 on failure.
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshAddInterface(
    mut env: JNIEnv,
    _class:  JClass,
    path:    JString,
    arg:     JString,
) -> jint {
    let name_rs: String = match env.get_string(&path) {
        Ok(s) => s.into(),
        Err(_) => return -1,
    };
    
    let arg_rs: Option<String> = if !arg.is_null() {
        env.get_string(&arg).ok().map(|s| s.into())
    } else {
        None
    };

    let (name_ptr, name_len) = (name_rs.as_ptr(), name_rs.len());
    let (arg_ptr, arg_len)   = match &arg_rs {
        Some(s) => (s.as_ptr(), s.len()),
        None    => (std::ptr::null(), 0),
    };

    unsafe { ffi::mesh_add_interface(name_ptr, name_len, arg_ptr, arg_len) }
}

// ── Data path ─────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshPushRx(
    mut env: JNIEnv,
    _class:  JClass,
    name:    JString,
    data:    JByteArray,
) -> jboolean {
    let name_str: String = match env.get_string(&name) {
        Ok(s)  => s.into(),
        Err(_) => return JNI_FALSE,
    };
    let bytes: Vec<u8> = match env.convert_byte_array(data) {
        Ok(b)  => b,
        Err(_) => return JNI_FALSE,
    };
    let ok = unsafe {
        ffi::mesh_push_rx(
            name_str.as_ptr(), name_str.len(),
            bytes.as_ptr(),    bytes.len(),
        )
    };
    if ok { JNI_TRUE } else { JNI_FALSE }
}

/// Returns the next outgoing packet for a named interface, or null if nothing to send.
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshPopTx(
    mut env: JNIEnv,
    _class:  JClass,
    name:    JString,
) -> jbyteArray {
    let name_str: String = match env.get_string(&name) {
        Ok(s)  => s.into(),
        Err(_) => return std::ptr::null_mut(),
    };

    let mut buf     = vec![0u8; 4096];
    let mut out_len = 0usize;

    let ok = unsafe {
        ffi::mesh_pop_tx(
            name_str.as_ptr(), name_str.len(),
            buf.as_mut_ptr(),  buf.len(),
            &mut out_len,
        )
    };

    if !ok || out_len == 0 {
        return std::ptr::null_mut();
    }

    buf.truncate(out_len);
    match env.byte_array_from_slice(&buf) {
        Ok(arr) => arr.into_raw(),
        Err(_)  => std::ptr::null_mut(),
    }
}

// ── Sending ───────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshSendTx(
    env:    JNIEnv,
    _class: JClass,
    tx_data: JByteArray,
) -> jboolean {
    let bytes: Vec<u8> = match env.convert_byte_array(tx_data) {
        Ok(b)  => b,
        Err(_) => return JNI_FALSE,
    };
    let ok = unsafe { ffi::mesh_send_tx(bytes.as_ptr(), bytes.len()) };
    if ok { JNI_TRUE } else { JNI_FALSE }
}

/// dest_hex: 32-char hex string (16 bytes)
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshSendTo(
    mut env:  JNIEnv,
    _class:   JClass,
    dest_hex: JString,
    payload:  JByteArray,
) -> jboolean {
    let hex_str: String = match env.get_string(&dest_hex) {
        Ok(s)  => s.into(),
        Err(_) => return JNI_FALSE,
    };
    let payload_bytes: Vec<u8> = match env.convert_byte_array(payload) {
        Ok(b)  => b,
        Err(_) => return JNI_FALSE,
    };
    let ok = unsafe {
        ffi::mesh_send_to(
            hex_str.as_ptr(),      hex_str.len(),
            payload_bytes.as_ptr(), payload_bytes.len(),
        )
    };
    if ok { JNI_TRUE } else { JNI_FALSE }
}

// ── Receive ───────────────────────────────────────────────────────────────────

/// Returns the next decoded inbound packet as a byte array, or null if none available.
/// First byte of the array is the dest_tag (0x00 = node, 0x01 = tx group).
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshPoll(
    mut env: JNIEnv,
    _class:  JClass,
) -> jbyteArray {
    let mut buf     = vec![0u8; 4096];
    let mut out_len = 0usize;

    let ok = unsafe { ffi::mesh_poll(buf.as_mut_ptr(), buf.len(), &mut out_len) };

    if !ok || out_len == 0 {
        return std::ptr::null_mut();
    }

    buf.truncate(out_len);
    match env.byte_array_from_slice(&buf) {
        Ok(arr) => arr.into_raw(),
        Err(_)  => std::ptr::null_mut(),
    }
}

// ── Identity ──────────────────────────────────────────────────────────────────

use jni::sys::jstring;

/// Returns our local destination hash as a 32-char hex String, or null.
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshLocalHash(
    mut env: JNIEnv,
    _class:  JClass,
) -> jstring {
    let mut buf = vec![0u8; 33];
    if unsafe { ffi::mesh_local_hash(buf.as_mut_ptr(), buf.len()) } {
        let hex = std::str::from_utf8(&buf[..32]).unwrap_or("");
        match env.new_string(hex) {
            Ok(s)  => s.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// Returns the GROUP tx relay hash as a 32-char hex String, or null.
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshTxGroupHash(
    mut env: JNIEnv,
    _class:  JClass,
) -> jstring {
    let mut buf = vec![0u8; 33];
    if unsafe { ffi::mesh_tx_group_hash(buf.as_mut_ptr(), buf.len()) } {
        let hex = std::str::from_utf8(&buf[..32]).unwrap_or("");
        match env.new_string(hex) {
            Ok(s)  => s.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

// ── Peer discovery ────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshPeerCount(
    _env:   JNIEnv,
    _class: JClass,
) -> jint {
    unsafe { ffi::mesh_peer_count() as jint }
}

/// Returns a peer's hash hex string, or null if index out of range.
/// App data (display name) is returned as a separate call to meshGetPeerAppData.
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshGetPeerHash(
    mut env: JNIEnv,
    _class:  JClass,
    index:   jint,
) -> jstring {
    let mut hash_buf     = vec![0u8; 33];
    let mut app_data_buf = vec![0u8; 256];
    let mut app_data_len = 0usize;

    let ok = unsafe {
        ffi::mesh_get_peer(
            index as u32,
            hash_buf.as_mut_ptr(),     hash_buf.len(),
            app_data_buf.as_mut_ptr(), app_data_buf.len(),
            &mut app_data_len,
        )
    };

    if !ok { return std::ptr::null_mut(); }

    let hex = std::str::from_utf8(&hash_buf[..32]).unwrap_or("");
    match env.new_string(hex) {
        Ok(s)  => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Returns a peer's app_data bytes, or null if index out of range or no app_data.
#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshGetPeerAppData(
    mut env: JNIEnv,
    _class:  JClass,
    index:   jint,
) -> jbyteArray {
    let mut hash_buf     = vec![0u8; 33];
    let mut app_data_buf = vec![0u8; 256];
    let mut app_data_len = 0usize;

    let ok = unsafe {
        ffi::mesh_get_peer(
            index as u32,
            hash_buf.as_mut_ptr(),     hash_buf.len(),
            app_data_buf.as_mut_ptr(), app_data_buf.len(),
            &mut app_data_len,
        )
    };

    if !ok || app_data_len == 0 { return std::ptr::null_mut(); }

    app_data_buf.truncate(app_data_len.min(256));
    match env.byte_array_from_slice(&app_data_buf) {
        Ok(arr) => arr.into_raw(),
        Err(_)  => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_reticulum_ReticulumModule_meshClearPeers(
    _env:   JNIEnv,
    _class: JClass,
) {
    unsafe { ffi::mesh_clear_peers() }
}
