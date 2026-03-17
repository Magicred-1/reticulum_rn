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
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    path::Path,
    sync::{Arc, Mutex},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::Notify;
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
        ContextFlag, DestinationType, Header, HeaderType, IfacFlag, Packet, PacketContext,
        PacketDataBuffer, PacketType, PropagationType,
    },
    transport::{Transport, TransportConfig},
};
use rusqlite::{params, Connection};
// use lxmf::message::Message as LxmfMessage;

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
        | ((h.context_flag   as u8) << 5)
        | ((h.propagation_type as u8) << 4)
        | ((h.destination_type as u8) << 2)
        | (h.packet_type     as u8)
}
/// Interface Modes as defined in the Reticulum Manual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceMode {
    Full,     // Default discovery/meshing
    Gateway,  // Full + path discovery on behalf of clients
    AccessPoint, // Quiet announces + path resolution helper
    Roaming,  // Faster path expiry for mobile nodes
    Boundary, // Interconnects significantly different segments
}

impl InterfaceMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "gw" | "gateway"      => Self::Gateway,
            "ap" | "access_point" => Self::AccessPoint,
            "roaming"             => Self::Roaming,
            "boundary"            => Self::Boundary,
            _                     => Self::Full,
        }
    }
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
    pub mode:     InterfaceMode,
}

impl QueuedDriver {
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue, mode: InterfaceMode) -> Self {
        Self { incoming, outgoing, mode }
    }

    pub async fn spawn<D>(context: InterfaceContext<D>)
    where
        D: AsRef<QueuedDriver> + Interface + Send + Sync + 'static,
    {
        let (incoming, outgoing, mode) = {
            let inner = context.inner.lock().unwrap();
            (inner.as_ref().incoming.clone(), inner.as_ref().outgoing.clone(), inner.as_ref().mode)
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
                        // OPTIMIZATION: If interface is in AP mode, 
                        // we can optionally suppress broadcasat announces here 
                        // to save native radio bandwidth/battery.
                        if mode == InterfaceMode::AccessPoint {
                            if msg.packet.header.packet_type == PacketType::Announce {
                                continue; // Don't wake up the radio for announces in AP mode
                            }
                        }
                        
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
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue, mode: InterfaceMode) -> Self {
        Self(QueuedDriver::new(incoming, outgoing, mode))
    }
    pub async fn spawn(ctx: InterfaceContext<BLEDriver>) { QueuedDriver::spawn(ctx).await; }
}
impl AsRef<QueuedDriver> for BLEDriver { fn as_ref(&self) -> &QueuedDriver { &self.0 } }
impl Interface for BLEDriver { fn mtu() -> usize { 512 } }

pub struct LoRaDriver(QueuedDriver);
impl LoRaDriver {
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue, mode: InterfaceMode) -> Self {
        Self(QueuedDriver::new(incoming, outgoing, mode))
    }
    pub async fn spawn(ctx: InterfaceContext<LoRaDriver>) { QueuedDriver::spawn(ctx).await; }
}
impl AsRef<QueuedDriver> for LoRaDriver { fn as_ref(&self) -> &QueuedDriver { &self.0 } }
impl Interface for LoRaDriver { fn mtu() -> usize { 235 } }



// ── Auto interface (UDP multicast — WiFi/LAN discovery) ──────────────────────

/// Default IPv6 multicast group for AutoInterface (all nodes link-local).
pub const AUTO_MULTICAST_GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
/// Default IPv4 multicast group for AutoInterface (all systems).
pub const AUTO_MULTICAST_GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 1);
/// Default port for AutoInterface (Reticulum standard).
pub const AUTO_PORT: u16 = 29716;

pub struct AutoDriver {
    pub incoming: ByteQueue,
    pub outgoing: ByteQueue,
    pub mode:     InterfaceMode,
}

impl AutoDriver {
    pub fn new(incoming: ByteQueue, outgoing: ByteQueue, mode: InterfaceMode) -> Self {
        Self { incoming, outgoing, mode }
    }

    /// Bind a UDP socket for multicast discovery.
    /// Prefers dual-stack or IPv4 if possible for maximum compatibility.
    async fn try_bind() -> std::io::Result<tokio::net::UdpSocket> {
        // Bind to any address on the standard port
        let socket = match tokio::net::UdpSocket::bind(format!("0.0.0.0:{}", AUTO_PORT)).await {
            Ok(s) => s,
            Err(_) => tokio::net::UdpSocket::bind("0.0.0.0:0").await?,
        };

        // Try to join both multicast groups
        let _ = socket.join_multicast_v4(AUTO_MULTICAST_GROUP_V4, Ipv4Addr::UNSPECIFIED);
        
        // Joining IPv6 multicast on dual-stack socket
        // Note: some platforms require binding [::] for ff02 join.
        // For mobile, we stick to IPv4 primarily as it's most reliable for LAN discovery.
        
        // Disable loopback
        let _ = socket.set_multicast_loop_v4(false);

        Ok(socket)
    }

    pub async fn spawn(context: InterfaceContext<AutoDriver>) {
        let (incoming, _outgoing, mode) = {
            let inner = context.inner.lock().unwrap();
            (inner.incoming.clone(), inner.outgoing.clone(), inner.mode)
        };
        let iface_address = context.channel.address;
        let stop          = context.channel.stop.clone();
        let cancel        = context.cancel.clone();
        let (rx_chan, mut tx_chan) = context.channel.split();

        // Attempt to bind the UDP multicast socket
        let socket = match Self::try_bind().await {
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

        let target = SocketAddr::V4(SocketAddrV4::new(AUTO_MULTICAST_GROUP_V4, AUTO_PORT));

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
                        // OPTIMIZATION: If interface is in AP mode,
                        // we can optionally suppress broadcasat announces here
                        // to save native radio bandwidth/battery.
                        if mode == InterfaceMode::AccessPoint {
                            if msg.packet.header.packet_type == PacketType::Announce {
                                continue; // Don't wake up the radio for announces in AP mode
                            }
                        }

                        let serialized = serialize_packet(&msg.packet);
                        if let Err(e) = socket.send_to(&serialized, target).await {
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
    pub mode:     InterfaceMode,
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
    pub rx:           Arc<Mutex<VecDeque<Vec<u8>>>>,
    running:          Arc<AtomicBool>,
    rt:               Option<tokio::runtime::Runtime>,
    cancel:           CancellationToken,
    /// Discovered peers — keyed by hex hash, updated on every announce.
    pub peers: PeerTable,
    /// Active transport handle. Use to send packets or query state.
    pub transport: Arc<Mutex<Option<Arc<Transport>>>>,
    /// Path to the SQLite database.
    pub db_path: Option<std::path::PathBuf>,
}

impl MeshNode {
    pub fn new(identity_path: &Path) -> Self {
        let identity = load_or_create_identity(identity_path);
        let mut db_path = identity_path.to_path_buf();
        db_path.set_extension("db");
        Self::from_identity_and_db(identity, Some(&db_path))
    }

    /// Create a node for testing with a random identity that is NOT saved to disk.
    pub fn new_in_memory() -> Self {
        Self::from_identity(PrivateIdentity::new_from_rand(OsRng))
    }

    fn from_identity(identity: PrivateIdentity) -> Self {
        Self::from_identity_and_db(identity, None)
    }

    fn from_identity_and_db(identity: PrivateIdentity, db_path: Option<&Path>) -> Self {
        // Derive our SINGLE address hash.
        let single = SingleInputDestination::new(
            identity.clone(),
            DestinationName::new("anon0mesh", "node"),
        );
        let local_hash    = single.desc.address_hash;
        let tx_group_hash = compute_group_hash("anon0mesh", "tx_relay");

        log::info!("[mesh] node identity derived");
        log::info!("[mesh] local_hash    = {}", local_hash);
        log::info!("[mesh] tx_group_hash = {}", tx_group_hash);

        Self {
            identity,
            local_hash,
            tx_group_hash,
            ifaces:  Vec::new(),
            rx:      Arc::new(Mutex::new(VecDeque::new())),
            running: Arc::new(AtomicBool::new(false)),
            rt:      None,
            cancel:  CancellationToken::new(),
            peers:   Arc::new(Mutex::new(std::collections::HashMap::new())),
            transport: Arc::new(Mutex::new(None)),
            db_path: db_path.map(|p| p.to_path_buf()),
        }
    }

    pub fn add_interface(&mut self, name: &'static str, _arg: Option<String>, mode: InterfaceMode) -> usize {
        let incoming = ByteQueue::new();
        let outgoing = ByteQueue::new();
        self.add_interface_raw(name, incoming, outgoing, mode)
    }

    /// Inject raw queues — used for testing "virtual cables" between nodes.
    pub fn add_interface_raw(&mut self, name: &'static str, incoming: ByteQueue, outgoing: ByteQueue, mode: InterfaceMode) -> usize {
        let idx = self.ifaces.len();
        self.ifaces.push(IfaceHandle { name, arg: None, incoming, outgoing, mode });
        idx
    }

    pub fn start(&mut self) -> bool {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("reticulum-rt")
            .enable_all()
            .build()
            .expect("tokio runtime");

        if rt.block_on(self.start_with_runtime()) {
            self.rt = Some(rt);
            true
        } else {
            false
        }
    }

    /// Start the node logic using an existing tokio runtime (useful for tests).
    pub async fn start_with_runtime(&self) -> bool {
        if self.running.swap(true, Ordering::SeqCst) { return false; }

        let identity      = self.identity.clone();
        let local_hash    = self.local_hash;
        let tx_group_hash = self.tx_group_hash;
        let cancel        = self.cancel.clone();
        let running       = Arc::clone(&self.running);
        let peers         = Arc::clone(&self.peers);
        let transport_out = Arc::clone(&self.transport);
        let rx            = self.rx.clone();
        let db_path       = self.db_path.clone();

        // Initialize DB schema
        if let Some(ref path) = db_path {
            if let Ok(conn) = Connection::open(path) {
                let _ = conn.execute(
                    "CREATE TABLE IF NOT EXISTS packets (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
                        dest_hash TEXT,
                        tag INTEGER,
                        data BLOB
                    )",
                    [],
                );
                let _ = conn.execute(
                    "CREATE TABLE IF NOT EXISTS lxmf_messages (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
                        message_id TEXT UNIQUE,
                        src_hash TEXT,
                        dest_hash TEXT,
                        content TEXT
                    )",
                    [],
                );
            }
        }

        let iface_queues: Vec<(&'static str, Option<String>, InterfaceMode, ByteQueue, ByteQueue)> = self.ifaces
            .iter()
            .map(|h| (h.name, h.arg.clone(), h.mode, h.incoming.clone(), h.outgoing.clone()))
            .collect();

        tokio::spawn(async move {
            let config   = TransportConfig::new("anon0mesh", &identity, true);
            let mut transport_raw = Transport::new(config);

            let dest = transport_raw.add_destination(
                identity.clone(),
                DestinationName::new("anon0mesh", "node")
            ).await;

            let transport = Arc::new(transport_raw);
            *transport_out.lock().unwrap() = Some(transport.clone());

            {
                let mgr_arc = transport.iface_manager();
                let mut mgr = mgr_arc.lock().await;
                for (name, arg, mode, inc, out) in iface_queues {
                    match name {
                        "ble"  => { mgr.spawn(BLEDriver::new(inc, out, mode),  BLEDriver::spawn);  }
                        "lora" => { mgr.spawn(LoRaDriver::new(inc, out, mode), LoRaDriver::spawn); }
                        "auto" => { mgr.spawn(AutoDriver::new(inc, out, mode), AutoDriver::spawn); }
                        "tcp_client" => { if let Some(t) = arg { mgr.spawn(TcpClient::new(t), TcpClient::spawn); } }
                        "tcp_server" => { if let Some(b) = arg { mgr.spawn(TcpServer::new(b, mgr_arc.clone()), TcpServer::spawn); } }
                        _ => {}
                    }
                }
            }

            {
                let transport = transport.clone();
                let cancel    = cancel.clone();
                let dest      = dest.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    loop {
                        transport.send_announce(&dest, None).await;
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_secs(5)) => {} // Fast for tests
                        }
                    }
                });
            }

            {
                let peers  = peers.clone();
                let cancel = cancel.clone();
                let mut announce_rx = transport.recv_announces().await;
                tokio::spawn(async move {
                    while let Ok(ann) = announce_rx.recv().await {
                        let s = format!("{}", ann.destination.lock().await.desc.address_hash);
                        let hash = s.trim_matches(|c| c == '/' || c == '<' || c == '>').trim_start_matches("0x").to_string();
                        peers.lock().unwrap().insert(hash.clone(), PeerInfo {
                            hash: hash.clone(),
                            app_data: ann.app_data.as_slice().to_vec(),
                        });
                        if cancel.is_cancelled() { break; }
                    }
                });
            }

            let mut iface_rx = transport.iface_rx();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    Ok(msg) = iface_rx.recv() => {
                        let pkt = msg.packet;
                        if pkt.header.packet_type != PacketType::Data { continue; }
                        let dest_tag = if pkt.destination == tx_group_hash { DEST_TX_GROUP }
                                       else if pkt.destination == local_hash { DEST_NODE }
                                       else { continue; };
                            let mut framed = Vec::with_capacity(1 + pkt.data.len());
                            framed.push(dest_tag);
                            framed.extend_from_slice(pkt.data.as_slice());
                            rx.lock().unwrap().push_back(framed);

                            // Persist to DB if enabled
                            if let Some(ref path) = db_path {
                                if let Ok(conn) = Connection::open(path) {
                                    let dest_hex = format!("{}", pkt.destination);
                                    let _ = conn.execute(
                                        "INSERT INTO packets (dest_hash, tag, data) VALUES (?1, ?2, ?3)",
                                        params![dest_hex, dest_tag as i32, pkt.data.as_slice()],
                                    );

                                    // Try to decode LXMF if it's a message
                                    if pkt.data.len() > 0 && pkt.data.as_slice()[0] == TAG_MESSAGE {
                                        // Simple heuristic for LXMF: try to parse the payload (skip TAG_MESSAGE byte)
                                        // In a real implementation, you'd check signatures/encryption.
                                        // For now, we just save the direct message content.
                                        let content = String::from_utf8_lossy(&pkt.data.as_slice()[1..]).to_string();
                                        let _ = conn.execute(
                                            "INSERT INTO lxmf_messages (src_hash, dest_hash, content) VALUES (?1, ?2, ?3)",
                                            params!["unknown", dest_hex, content],
                                        );
                                    }
                                }
                            }
                        }
                }
            }

            running.store(false, Ordering::SeqCst);
        });
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

    pub fn try_recv(&self) -> Option<Vec<u8>> {
        self.rx.lock().unwrap().pop_front()
    }

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
                context_flag: ContextFlag::Unset,
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
                context_flag: ContextFlag::Unset,
            },
            ifac: None, destination: dest, transport: None,
            context: PacketContext::None, data,
        };
        let serialized = serialize_packet(&pkt);
        for h in &self.ifaces { h.outgoing.push(serialized.clone()); }
        true
    }

    pub fn tx_group_hash_hex(&self) -> String { 
        let s = format!("{}", self.tx_group_hash);
        s.trim_matches(|c| c == '/' || c == '<' || c == '>').trim_start_matches("0x").to_string()
    }
    pub fn local_hash_hex(&self)    -> String { 
        let s = format!("{}", self.local_hash);
        s.trim_matches(|c| c == '/' || c == '<' || c == '>').trim_start_matches("0x").to_string()
    }

    /// Fetch historical messages from SQLite.
    pub fn fetch_messages(&self, limit: usize) -> Vec<serde_json::Value> {
        let mut results = Vec::new();
        if let Some(ref path) = self.db_path {
            if let Ok(conn) = Connection::open(path) {
                let mut stmt = conn.prepare(
                    "SELECT timestamp, src_hash, dest_hash, content FROM lxmf_messages ORDER BY timestamp DESC LIMIT ?"
                ).unwrap();
                let rows = stmt.query_map([limit], |row| {
                    Ok(serde_json::json!({
                        "timestamp": row.get::<_, String>(0)?,
                        "src_hash":  row.get::<_, String>(1)?,
                        "dest_hash": row.get::<_, String>(2)?,
                        "content":   row.get::<_, String>(3)?,
                    }))
                }).unwrap();

                for row in rows {
                    if let Ok(val) = row {
                        results.push(val);
                    }
                }
            }
        }
        results
    }
}

impl Drop for MeshNode {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interface_mode_parsing() {
        assert_eq!(InterfaceMode::from_str("full"), InterfaceMode::Full);
        assert_eq!(InterfaceMode::from_str("gw"), InterfaceMode::Gateway);
        assert_eq!(InterfaceMode::from_str("ap"), InterfaceMode::AccessPoint);
        assert_eq!(InterfaceMode::from_str("roaming"), InterfaceMode::Roaming);
        assert_eq!(InterfaceMode::from_str("boundary"), InterfaceMode::Boundary);
        assert_eq!(InterfaceMode::from_str("unknown"), InterfaceMode::Full);
    }

    #[test]
    fn test_identity_and_hashing() {
        let id_raw = PrivateIdentity::new_from_rand(OsRng);
        
        let single = SingleInputDestination::new(
            id_raw.clone(),
            DestinationName::new("anon0mesh", "node"),
        );
        let hash = single.desc.address_hash;
        // Verify it returns a valid hash hex (32 or 34 with 0x)
        let s = format!("{}", hash);
        assert!(s.len() == 32 || s.len() == 34);
    }

    #[test]
    fn test_node_communication() {
        let _ = env_logger::builder().is_test(true).try_init();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Setup a "virtual cable" between two nodes
            let a_to_b = ByteQueue::new();
            let b_to_a = ByteQueue::new();

            // Node A
            let mut node_a = MeshNode::new_in_memory();
            // Use 'ble' so start_with_runtime spawns the driver
            node_a.add_interface_raw("ble", a_to_b.clone(), b_to_a.clone(), InterfaceMode::Full);
            assert!(node_a.start_with_runtime().await);

            // Node B
            let mut node_b = MeshNode::new_in_memory();
            node_b.add_interface_raw("ble", b_to_a.clone(), a_to_b.clone(), InterfaceMode::Full);
            assert!(node_b.start_with_runtime().await);

            let b_hash = node_b.local_hash_hex();

            // Wait for node A to discover node B via automated announce
            let mut discovered = false;
            for _ in 0..150 {
                if node_a.peers.lock().unwrap().contains_key(&b_hash) {
                    discovered = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(discovered, "Node A failed to discover Node B via announce (peers: {:?})", node_a.peers.lock().unwrap().keys());

            // Send a message from A to B
            let test_msg = b"hello reticulum hookup";
            node_a.send_message(&b_hash, test_msg);

            // Poll Node B to see if it received the message
            let mut received = false;
            for _ in 0..100 {
                if let Some(data) = node_b.try_recv() {
                    // Packet format: [dest_tag] [payload]
                    // Tag 0x00 is SINGLE dest (messages)
                    if data[0] == 0x00 && &data[2..] == test_msg {
                        received = true;
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert!(received, "Node B failed to receive message from Node A");
        });
    }
}
