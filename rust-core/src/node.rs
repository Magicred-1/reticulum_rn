//! MeshNode — built against the real Reticulum-rs 0.1.0 public API.
//!
//! Key corrections from the previous version:
//!
//! - `Identity` is the PUBLIC half only; `PrivateIdentity` holds the keypair.
//! - `Destination` is generic; use the `SingleInputDestination` / `PlainInputDestination` aliases.
//! - `Transport` is fully async/tokio — NOT thread + std::sync.
//! - `Interface` trait has only `fn mtu() -> usize` (static method).
//! - Interfaces communicate via tokio mpsc channels, not read()/write().
//! - `reticulum::serde` is a private module; packet serialization is done manually here.
//! - Receiving uses `transport.iface_rx()` → broadcast::Receiver<RxMessage>.

use rand_core::OsRng;
use std::{
    collections::VecDeque,
    net::{Ipv6Addr, SocketAddrV6},
    path::Path,
    sync::{Arc, Mutex},
    sync::mpsc::{channel, Receiver, Sender},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::{broadcast, Notify};
use tokio_util::sync::CancellationToken;

use reticulum::{
    buffer::InputBuffer,
    destination::{DestinationName, PlainInputDestination, SingleInputDestination},
    hash::AddressHash,
    identity::{EmptyIdentity, PrivateIdentity},
    iface::{
        tcp_client::TcpClient,
        tcp_server::TcpServer,
        Interface, InterfaceContext, RxMessage,
    },
    packet::{
        DestinationType, Header, HeaderType, IfacFlag, Packet, PacketContext,
        PacketDataBuffer, PacketType, PropagationType,
    },
    transport::{Transport, TransportConfig},
};

// ── Payload type tags ─────────────────────────────────────────────────────────

pub const TAG_MESSAGE:   u8 = 0x00; // chat / arbitrary message
pub const TAG_SOLANA_TX: u8 = 0x01; // Solana durable-nonce transaction

// ── dest_tag framing (first byte pushed to JS rx queue) ───────────────────────
pub const DEST_NODE:     u8 = 0x00; // addressed to our SINGLE destination
pub const DEST_TX_GROUP: u8 = 0x01; // addressed to the GROUP relay destination

// ── Manual packet serialization ───────────────────────────────────────────────
// `reticulum::serde` is `mod serde` (private), so the `Serialize` trait is not
// accessible from outside the crate. We replicate the wire format here.
// Format: [meta(1)] [hops(1)] [transport_hash(16) if Type2] [dest(16)] [ctx(1)] [data...]

fn header_to_meta(h: &Header) -> u8 {
    ((h.ifac_flag        as u8) << 7)
        | ((h.header_type    as u8) << 6)
        | ((h.propagation_type as u8) << 4)
        | ((h.destination_type as u8) << 2)
        | (h.packet_type     as u8)
}

pub fn serialize_packet(p: &Packet) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.push(header_to_meta(&p.header));
    out.push(p.header.hops);
    if p.header.header_type == HeaderType::Type2 {
        if let Some(t) = p.transport {
            out.extend_from_slice(t.as_slice());
        }
    }
    out.extend_from_slice(p.destination.as_slice());
    out.push(p.context as u8);
    out.extend_from_slice(p.data.as_slice());
    out
}

// ── Byte queue — shared between sync FFI and async interface tasks ─────────────

#[derive(Clone)]
pub struct ByteQueue {
    inner:  Arc<Mutex<VecDeque<Vec<u8>>>>,
    notify: Arc<Notify>,
}

impl ByteQueue {
    pub fn new() -> Self {
        Self {
            inner:  Arc::new(Mutex::new(VecDeque::new())),
            notify: Arc::new(Notify::new()),
        }
    }
    pub fn push(&self, data: Vec<u8>) {
        self.inner.lock().unwrap().push_back(data);
        self.notify.notify_one();
    }
    pub fn pop(&self) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().pop_front()
    }
    /// Wait for new data to be available (async).
    pub async fn wait(&self) {
        self.notify.notified().await;
    }
}

// ── Queued Driver (generic for BLE/LoRa/Native-passed bytes) ───────────────────

pub struct QueuedDriver {
    pub incoming: ByteQueue, // native → Reticulum
    pub outgoing: ByteQueue, // Reticulum → native
}

impl QueuedDriver {
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue) -> Self {
        Self { incoming, outgoing }
    }

    pub async fn spawn<D>(context: InterfaceContext<D>)
    where
        D: AsRef<QueuedDriver> + Interface + Send + Sync + 'static,
    {
        let (incoming, outgoing) = {
            let inner = context.inner.lock().unwrap();
            (inner.as_ref().incoming.clone(), inner.as_ref().outgoing.clone())
        };
        let iface_address = context.channel.address;
        let stop          = context.channel.stop.clone();
        let cancel        = context.cancel.clone();
        let (rx_chan, mut tx_chan) = context.channel.split();

        // RX task: wait for notification → drain queue → deserialize → send to transport
        let rx_task = {
            let (cancel, stop) = (cancel.clone(), stop.clone());
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = stop.cancelled()   => break,
                        _ = incoming.wait() => {
                            while let Some(raw) = incoming.pop() {
                                let mut buf = InputBuffer::new(&raw);
                                if let Ok(pkt) = Packet::deserialize(&mut buf) {
                                    let _ = rx_chan.send(RxMessage { 
                                        address: iface_address, 
                                        packet: pkt 
                                    }).await;
                                }
                            }
                        }
                    }
                }
            })
        };

        // TX task: receive from transport → serialize → push to outgoing queue.
        let stop_tx = stop.clone();
        let tx_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled()  => break,
                    _ = stop_tx.cancelled() => break,
                    Some(msg) = tx_chan.recv() => {
                        outgoing.push(serialize_packet(&msg.packet));
                    }
                }
            }
        });

        let _ = tokio::join!(rx_task, tx_task);
        stop.cancel();
    }
}

pub struct BLEDriver(QueuedDriver);
impl BLEDriver {
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue) -> Self {
        Self(QueuedDriver::new(incoming, outgoing))
    }
    pub async fn spawn(ctx: InterfaceContext<BLEDriver>) { QueuedDriver::spawn(ctx).await; }
}
impl AsRef<QueuedDriver> for BLEDriver { fn as_ref(&self) -> &QueuedDriver { &self.0 } }
impl Interface for BLEDriver { fn mtu() -> usize { 512 } }

pub struct LoRaDriver(QueuedDriver);
impl LoRaDriver {
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue) -> Self {
        Self(QueuedDriver::new(incoming, outgoing))
    }
    pub async fn spawn(ctx: InterfaceContext<LoRaDriver>) { QueuedDriver::spawn(ctx).await; }
}
impl AsRef<QueuedDriver> for LoRaDriver { fn as_ref(&self) -> &QueuedDriver { &self.0 } }
impl Interface for LoRaDriver { fn mtu() -> usize { 235 } }



// ── Auto interface (UDP multicast — WiFi/LAN discovery) ──────────────────────

/// Default multicast group for AutoInterface (IPv6 link-local all nodes).
pub const AUTO_MULTICAST_GROUP: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
/// Default port for AutoInterface (Reticulum standard).
pub const AUTO_PORT: u16 = 29716;

pub struct AutoDriver {
    pub incoming: ByteQueue,
    pub outgoing: ByteQueue,
}

impl AutoDriver {
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue) -> Self {
        Self { incoming, outgoing }
    }

    /// Bind a UDP socket for multicast discovery.
    /// Falls back to an ephemeral port if the default is in use.
    async fn bind_multicast() -> std::io::Result<tokio::net::UdpSocket> {
        let socket = match tokio::net::UdpSocket::bind(
            SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, AUTO_PORT, 0, 0),
        ).await {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[mesh] AutoInterface (IPv6) port {} in use, trying ephemeral: {}",
                    AUTO_PORT, e
                );
                tokio::net::UdpSocket::bind(
                    SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0),
                ).await?
            }
        };

        // Join multicast group (best-effort — may fail without entitlement on iOS)
        match socket.join_multicast_v6(&AUTO_MULTICAST_GROUP, 0) {
            Ok(_) => log::info!(
                "[mesh] AutoInterface joined IPv6 multicast {}:{}",
                AUTO_MULTICAST_GROUP, AUTO_PORT
            ),
            Err(e) => log::warn!(
                "[mesh] AutoInterface IPv6 multicast join failed: {} — UDP send-only mode", e
            ),
        }

        // Disable loopback so we don't receive our own packets
        let _ = socket.set_multicast_loop_v6(false);

        Ok(socket)
    }

    pub async fn spawn(context: InterfaceContext<AutoDriver>) {
        let incoming      = context.inner.lock().unwrap().incoming.clone();
        let _outgoing     = context.inner.lock().unwrap().outgoing.clone(); // reserved for observability
        let iface_address = context.channel.address;
        let stop          = context.channel.stop.clone();
        let cancel        = context.cancel.clone();
        let (rx_chan, mut tx_chan) = context.channel.split();

        // Attempt to bind the UDP multicast socket
        let socket = match Self::bind_multicast().await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                log::error!("[mesh] AutoInterface failed to bind: {}", e);
                // Fallback: run as a queue-only interface (same pattern as BLE/LoRa)
                let (cancel2, stop2, incoming2) = (cancel.clone(), stop.clone(), incoming.clone());
                let rx_task = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = cancel2.cancelled() => break,
                            _ = stop2.cancelled()   => break,
                            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                                while let Some(raw) = incoming2.pop() {
                                    let mut buf = InputBuffer::new(&raw);
                                    if let Ok(pkt) = Packet::deserialize(&mut buf) {
                                        let _ = rx_chan.send(RxMessage {
                                            address: iface_address, packet: pkt,
                                        }).await;
                                    }
                                }
                            }
                        }
                    }
                });
                let _ = rx_task.await;
                stop.cancel();
                return;
            }
        };

        let multicast_target = SocketAddrV6::new(AUTO_MULTICAST_GROUP, AUTO_PORT, 0, 0);

        // rx_task: recv from UDP socket + poll incoming ByteQueue → transport
        let rx_task = {
            let (cancel, stop, socket) = (cancel.clone(), stop.clone(), socket.clone());
            tokio::spawn(async move {
                let mut udp_buf = vec![0u8; 2048];
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = stop.cancelled()   => break,
                        // UDP multicast receive
                        result = socket.recv_from(&mut udp_buf) => {
                            match result {
                                Ok((len, _src)) => {
                                    let mut buf = InputBuffer::new(&udp_buf[..len]);
                                    if let Ok(pkt) = Packet::deserialize(&mut buf) {
                                        let _ = rx_chan.send(RxMessage {
                                            address: iface_address, packet: pkt,
                                        }).await;
                                    }
                                }
                                Err(e) => {
                                    log::warn!("[mesh] AutoInterface recv error: {}", e);
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            }
                        }
                        // Also drain the ByteQueue for manually injected data
                        _ = incoming.wait() => {
                            while let Some(raw) = incoming.pop() {
                                let mut buf = InputBuffer::new(&raw);
                                if let Ok(pkt) = Packet::deserialize(&mut buf) {
                                    let _ = rx_chan.send(RxMessage {
                                        address: iface_address, packet: pkt,
                                    }).await;
                                }
                            }
                        }
                    }
                }
            })
        };

        // tx_task: transport → UDP multicast (self-contained, no ByteQueue push)
        let stop_tx = stop.clone();
        let tx_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled()  => break,
                    _ = stop_tx.cancelled() => break,
                    Some(msg) = tx_chan.recv() => {
                        let serialized = serialize_packet(&msg.packet);
                        if let Err(e) = socket.send_to(&serialized, multicast_target).await {
                            log::warn!("[mesh] AutoInterface send error: {}", e);
                        }
                    }
                }
            }
        });

        let _ = tokio::join!(rx_task, tx_task);
        stop.cancel();
    }
}

impl Interface for AutoDriver {
    fn mtu() -> usize { 1200 } // safe UDP payload — no fragmentation on most LANs
}

// ── Interface registration handle ─────────────────────────────────────────────

pub struct IfaceHandle {
    pub name:     &'static str,
    pub arg:      Option<String>, // e.g. "1.2.3.4:4242"
    pub incoming: ByteQueue,  // FFI pushes bytes here  (native → Rust)
    pub outgoing: ByteQueue,  // FFI pops bytes here    (Rust → native)
}

// ── Identity persistence ───────────────────────────────────────────────────────

pub fn load_or_create_identity(path: &Path) -> PrivateIdentity {
    if path.exists() {
        if let Ok(hex) = std::fs::read_to_string(path) {
            if let Ok(id) = PrivateIdentity::new_from_hex_string(hex.trim()) {
                return id;
            }
        }
        log::warn!("[mesh] corrupt identity at {:?}, regenerating", path);
    }
    let id = PrivateIdentity::new_from_rand(OsRng);
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let _ = std::fs::write(path, id.to_hex_string());
    id
}

/// GROUP dest hash = address hash of a `PlainInputDestination` with `EmptyIdentity`.
/// `EmptyIdentity::as_address_hash_slice()` returns `&[]`, so the hash is derived
/// purely from the name — identical on every anon0mesh node worldwide.
fn compute_group_hash(app: &str, aspects: &str) -> AddressHash {
    PlainInputDestination::new(EmptyIdentity, DestinationName::new(app, aspects))
        .desc.address_hash
}


// ── Peer table ────────────────────────────────────────────────────────────────

/// Information about a discovered peer, derived from their announce packet.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// 32-char lowercase hex address hash — the peer's SINGLE destination.
    pub hash:      String,
    /// Optional display name / app_data bytes from the announce.
    pub app_data:  Vec<u8>,
}

/// Shared peer table — written by the async announce listener, read by FFI.
pub type PeerTable = Arc<Mutex<std::collections::HashMap<String, PeerInfo>>>;

// ── MeshNode ──────────────────────────────────────────────────────────────────

pub struct MeshNode {
    /// Our persistent identity.
    pub identity:     PrivateIdentity,
    pub local_hash:   AddressHash,  // our SINGLE destination hash
    pub tx_group_hash: AddressHash, // GROUP relay hash — same on every device
    pub ifaces:       Vec<IfaceHandle>,
    rx:               Receiver<Vec<u8>>,
    tx:               Sender<Vec<u8>>,
    running:          Arc<AtomicBool>,
    rt:               Option<tokio::runtime::Runtime>,
    cancel:           CancellationToken,
    /// Discovered peers — keyed by hex hash, updated on every announce.
    pub peers: PeerTable,
    /// Active transport handle. Use to send packets or query state.
    pub transport: Arc<Mutex<Option<Arc<Transport>>>>,
}

impl MeshNode {
    pub fn new(identity_path: &Path) -> Self {
        let (tx, rx)  = channel::<Vec<u8>>();
        let identity  = load_or_create_identity(identity_path);

        // Derive our SINGLE address hash.
        let single = SingleInputDestination::new(
            identity.clone(),
            DestinationName::new("anon0mesh", "node"),
        );
        let local_hash    = single.desc.address_hash;
        let tx_group_hash = compute_group_hash("anon0mesh", "tx_relay");

        log::info!("[mesh] local_hash    = {}", local_hash);
        log::info!("[mesh] tx_group_hash = {}", tx_group_hash);

        Self {
            identity,
            local_hash,
            tx_group_hash,
            ifaces:  Vec::new(),
            rx,
            tx,
            running:   Arc::new(AtomicBool::new(false)),
            rt:        None,
            cancel:    CancellationToken::new(),
            peers:     Arc::new(Mutex::new(std::collections::HashMap::new())),
            transport: Arc::new(Mutex::new(None)),
        }
    }

    pub fn add_interface(&mut self, name: &'static str, arg: Option<String>) -> usize {
        self.ifaces.push(IfaceHandle {
            name,
            arg,
            incoming: ByteQueue::new(),
            outgoing: ByteQueue::new(),
        });
        self.ifaces.len() - 1
    }

    pub fn start(&mut self) -> bool {
        if self.running.swap(true, Ordering::SeqCst) { return false; }

        let identity      = self.identity.clone();
        let local_hash    = self.local_hash;
        let tx_group_hash = self.tx_group_hash;
        let tx_chan        = self.tx.clone();
        let cancel        = self.cancel.clone();
        let running       = Arc::clone(&self.running);
        let peers         = Arc::clone(&self.peers);
        let transport_out = Arc::clone(&self.transport);

        // Snapshot interface queues before the move
        let iface_queues: Vec<(&'static str, Option<String>, ByteQueue, ByteQueue)> = self.ifaces
            .iter()
            .map(|h| (h.name, h.arg.clone(), h.incoming.clone(), h.outgoing.clone()))
            .collect();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("reticulum-rt")
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.spawn(async move {
            let config   = TransportConfig::new("anon0mesh", &identity, /*broadcast=*/true);
            let mut transport_raw = Transport::new(config);

            // Register our SINGLE destination for inbound data BEFORE wrapping in Arc
            let dest = transport_raw.add_destination(
                identity.clone(),
                DestinationName::new("anon0mesh", "node")
            ).await;

            let transport = Arc::new(transport_raw);
            *transport_out.lock().unwrap() = Some(transport.clone());

            // Register interfaces
            {
                let mgr_arc = transport.iface_manager();
                let mut mgr = mgr_arc.lock().await;
                for (name, arg, inc, out) in iface_queues {
                    match name {
                        "ble"  => { mgr.spawn(BLEDriver::new(inc, out),  BLEDriver::spawn);  }
                        "lora" => { mgr.spawn(LoRaDriver::new(inc, out), LoRaDriver::spawn); }
                        "auto" => { mgr.spawn(AutoDriver::new(inc, out), AutoDriver::spawn); }
                        "tcp_client" => {
                            if let Some(target) = arg {
                                mgr.spawn(TcpClient::new(target), TcpClient::spawn);
                            }
                        }
                        "tcp_server" => {
                            if let Some(bind) = arg {
                                mgr.spawn(TcpServer::new(bind, mgr_arc.clone()), TcpServer::spawn);
                            }
                        }
                        other  => log::warn!("[mesh] unknown iface: {}", other),
                    }
                }
            }

            // Destination already registered above.


            // Periodic announce
            {
                let transport = transport.clone();
                let cancel    = cancel.clone();
                let dest      = dest.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    loop {
                        transport.send_announce(&dest, None).await;
                        log::debug!("[mesh] announced");
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_secs(300)) => {}
                        }
                    }
                });
            }

            // Peer discovery — subscribe to announce events from the transport.
            // Every announce from another anon0mesh node updates the peer table.
            {
                let peers  = peers.clone();
                let cancel = cancel.clone();
                let mut announce_rx = transport.recv_announces().await;
                tokio::spawn(async move {

                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            Ok(event) = announce_rx.recv() => {
                                let dest = event.destination.lock().await;
                                let hash = dest.desc.address_hash.to_hex_string();
                                let app_data = event.app_data.as_slice().to_vec();
                                log::debug!("[mesh] peer seen: {}", hash);

                                let info = PeerInfo { hash: hash.clone(), app_data };
                                peers.lock().unwrap().insert(hash, info);
                            }
                        }
                    }
                });
            }

            // Receive loop — data packets addressed to us
            let mut iface_rx: broadcast::Receiver<RxMessage> = transport.iface_rx();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    Ok(msg) = iface_rx.recv() => {
                        let pkt = msg.packet;
                        if pkt.header.packet_type != PacketType::Data { continue; }

                        let dest_tag = if pkt.destination == tx_group_hash {
                            DEST_TX_GROUP
                        } else if pkt.destination == local_hash {
                            DEST_NODE
                        } else {
                            continue;
                        };

                        let mut framed = Vec::with_capacity(1 + pkt.data.len());
                        framed.push(dest_tag);
                        framed.extend_from_slice(pkt.data.as_slice());
                        let _ = tx_chan.send(framed);
                    }
                }
            }

            running.store(false, Ordering::SeqCst);
            log::info!("[mesh] stopped");
        });

        self.rt = Some(rt);
        true
    }

    pub fn stop(&self)       { self.cancel.cancel(); }
    pub fn is_running(&self) -> bool { self.running.load(Ordering::SeqCst) }

    /// Push raw bytes from native radio into a named interface. Called from Swift/Kotlin.
    pub fn push_rx(&self, iface_name: &str, data: Vec<u8>) -> bool {
        match self.ifaces.iter().find(|h| h.name == iface_name) {
            Some(h) => { h.incoming.push(data); true }
            None    => false,
        }
    }

    /// Pop one outgoing packet for a named interface. Called from Swift/Kotlin TX path.
    pub fn pop_tx(&self, iface_name: &str) -> Option<Vec<u8>> {
        self.ifaces.iter().find(|h| h.name == iface_name).and_then(|h| h.outgoing.pop())
    }

    pub fn try_recv(&self) -> Option<Vec<u8>> { self.rx.try_recv().ok() }

    /// Number of reachable peers currently in the peer table.
    pub fn peer_count(&self) -> usize {
        self.peers.lock().unwrap().len()
    }

    /// Return a snapshot of all known peers, sorted by hash for stable ordering.
    pub fn peer_list(&self) -> Vec<PeerInfo> {
        let table = self.peers.lock().unwrap();
        let mut peers: Vec<PeerInfo> = table.values().cloned().collect();
        peers.sort_by(|a, b| a.hash.cmp(&b.hash));
        peers
    }


    /// Clear the entire peer table.
    pub fn clear_peers(&self) {
        self.peers.lock().unwrap().clear();
    }

    /// Send a Solana tx through the GROUP relay.
    pub fn send_tx(&self, tx_bytes: &[u8]) -> bool {
        if !self.is_running() { return false; }
        let mut data = PacketDataBuffer::new();
        data.safe_write(&[TAG_SOLANA_TX]);
        data.safe_write(tx_bytes);
        let pkt = Packet {
            header: Header {
                ifac_flag: IfacFlag::Open, header_type: HeaderType::Type1,
                propagation_type: PropagationType::Broadcast,
                destination_type: DestinationType::Group,
                packet_type: PacketType::Data, hops: 0,
            },
            ifac: None, destination: self.tx_group_hash, transport: None,
            context: PacketContext::None, data,
        };
        let serialized = serialize_packet(&pkt);
        for h in &self.ifaces { h.outgoing.push(serialized.clone()); }
        true
    }

    /// Send a direct message to a peer by hex address hash.
    pub fn send_message(&self, dest_hex: &str, message: &[u8]) -> bool {
        let dest = match AddressHash::new_from_hex_string(dest_hex) {
            Ok(h) => h, Err(_) => return false,
        };
        let mut data = PacketDataBuffer::new();
        data.safe_write(&[TAG_MESSAGE]);
        data.safe_write(message);
        let pkt = Packet {
            header: Header {
                ifac_flag: IfacFlag::Open, header_type: HeaderType::Type1,
                propagation_type: PropagationType::Broadcast,
                destination_type: DestinationType::Single,
                packet_type: PacketType::Data, hops: 0,
            },
            ifac: None, destination: dest, transport: None,
            context: PacketContext::None, data,
        };
        let serialized = serialize_packet(&pkt);
        for h in &self.ifaces { h.outgoing.push(serialized.clone()); }
        true
    }

    pub fn tx_group_hash_hex(&self) -> String { self.tx_group_hash.to_hex_string() }
    pub fn local_hash_hex(&self)    -> String { self.local_hash.to_hex_string() }
}

impl Drop for MeshNode {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_millis(500));
        }
    }
}
