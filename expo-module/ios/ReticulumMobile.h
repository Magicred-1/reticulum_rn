#pragma once
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Lifecycle
bool mesh_init(const uint8_t *path_ptr, size_t path_len);
bool mesh_start(void);
void mesh_stop(void);
bool mesh_is_running(void);

// Interface registration (call before mesh_start)
// Returns interface index >= 0 on success, -1 on failure.
// Supported names: "ble", "lora", "auto", "tcp_client", "tcp_server"
// arg_ptr is optional and used for TCP addresses (e.g. "1.2.3.4:4242")
int32_t mesh_add_interface(const uint8_t *name_ptr, size_t name_len,
                           const uint8_t *arg_ptr,  size_t arg_len,
                           const uint8_t *mode_ptr, size_t mode_len);

// Native radio I/O
// Push bytes received from BLE/LoRa into the named interface.
bool mesh_push_rx(const uint8_t *name_ptr, size_t name_len,
                  const uint8_t *data_ptr, size_t data_len);
// Pop one outgoing packet for a named interface (bytes to transmit over BLE/LoRa).
// Returns true with out_len=0 if nothing to send.
bool mesh_pop_tx(const uint8_t *name_ptr, size_t name_len,
                 uint8_t *buf, size_t buf_len, size_t *out_len);

// Sending
// Send Solana tx through the GROUP relay — no path needed.
bool mesh_send_tx(const uint8_t *tx_ptr, size_t tx_len);
// Send a direct message to a peer (dest_hex = 32 hex chars).
bool mesh_send_to(const uint8_t *dest_hex_ptr, size_t dest_hex_len,
                  const uint8_t *payload_ptr,  size_t payload_len);

// Receive poll
// Wire format: [dest_tag(1)] [payload...]
//   dest_tag 0x00 = SINGLE (our node)  → message
//   dest_tag 0x01 = GROUP  (tx relay)  → Solana tx
// Returns true with out_len=0 if no packet available.
bool mesh_poll(uint8_t *buf, size_t buf_len, size_t *out_len);

// Identity — buf must hold at least 33 bytes (32 hex + NUL)
bool mesh_local_hash(uint8_t *buf, size_t buf_len);
bool mesh_tx_group_hash(uint8_t *buf, size_t buf_len);

// Peer discovery
// Returns the number of known peers.
uint32_t mesh_peer_count(void);

// Read one peer by index (0-based, stable-sorted by hash hex).
// hash_buf must hold ≥ 33 bytes (32 hex + NUL).
// app_data_buf receives the raw announce app_data (display name, etc).
// out_app_data_len receives the full app_data byte count.
// Returns false if index out of range or node not initialised.
bool mesh_get_peer(uint32_t index,
                   uint8_t *hash_buf,     size_t hash_buf_len,
                   uint8_t *app_data_buf, size_t app_data_buf_len,
                   size_t *out_app_data_len);

// Remove all peers from the table.
void mesh_clear_peers(void);

// Fetch historical messages from SQLite. Returns JSON array string in buf.
bool mesh_fetch_messages(uint32_t limit, uint8_t *buf, size_t buf_len, size_t *out_len);

#ifdef __cplusplus
}
#endif
