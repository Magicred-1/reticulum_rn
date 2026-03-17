import {
  requireNativeModule,
  type EventSubscription,
} from 'expo-modules-core';

// ── Native module interface ───────────────────────────────────────────────────

type ReticulumModuleType = {
  // Lifecycle
  init(identityPath: string): Promise<boolean>;
  start(): Promise<boolean>;
  stop(): void;
  isRunning(): boolean;
  // Interfaces
  addInterface(name: string, arg: string): number;
  pushRx(ifaceName: string, data: number[]): boolean;
  // Sending
  sendTx(txBytes: number[]): Promise<boolean>;
  sendTo(destHex: string, payload: number[]): Promise<boolean>;
  txGroupHash(): Promise<string>;
  localHash(): Promise<string>;
  // Peers
  peerCount(): number;
  peers(): Array<{ hash: string; appData: number[] }>;
  clearPeers(): void;
  fetchMessages(limit: number): Promise<string>;
  // Events
  addListener(eventName: string, listener: (...args: any[]) => void): EventSubscription;
};

const ReticulumNative = requireNativeModule<ReticulumModuleType>('ReticulumModule');

// ── Types ─────────────────────────────────────────────────────────────────────

export interface PacketReceivedEvent {
  iface: string;
  data: number[];
}
export interface TxReceivedEvent { data: number[]; }
export interface MeshStateEvent { running: boolean; }
export interface PathFoundEvent { destHash: string; }

/** Fires when Rust has bytes ready for a physical radio. */
export interface OutgoingPacketEvent {
  iface: string;    // "ble" | "lora" | "auto"
  data: number[];  // write these bytes to the radio characteristic / serial port
}

// Re-export EventSubscription for consumers
export type { EventSubscription };

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/**
 * Initialise the Rust node with a path to persistent identity storage.
 * @param identityPath  e.g. `${documentDirectory}reticulum/identity`
 */
export async function init(identityPath: string): Promise<boolean> {
  return ReticulumNative.init(identityPath);
}

export async function start(): Promise<boolean> {
  return ReticulumNative.start();
}

export function stop(): void { ReticulumNative.stop(); }

export function isRunning(): boolean { return ReticulumNative.isRunning(); }

// ── Interfaces ────────────────────────────────────────────────────────────────

/**
 * Register a transport interface. Must call before `start()`.
 * Returns the interface index (>= 0) or -1 on failure.
 *
 * - `"ble"`:  BLE radio — native layer handles I/O via onOutgoing / pushRx
 * - `"lora"`: LoRa radio — native layer handles I/O via onOutgoing / pushRx
 * - `"auto"`: UDP multicast — self-contained in Rust, auto-discovers LAN peers
 * - `"tcp_client"`: TCP Client — connects to a remote Reticulum hub (arg: "ip:port")
 * - `"tcp_server"`: TCP Server — listens for incoming Reticulum links (arg: "ip:port")
 */
export function addInterface(name: 'ble' | 'lora' | 'auto' | 'tcp_client' | 'tcp_server', arg?: string): number {
  return ReticulumNative.addInterface(name, arg ?? "");
}

/**
 * Push raw bytes from a native BLE/LoRa callback into the named interface.
 */
export function pushRx(ifaceName: string, data: Uint8Array): boolean {
  return ReticulumNative.pushRx(ifaceName, Array.from(data));
}

// ── Sending ───────────────────────────────────────────────────────────────────

/**
 * Send a Solana transaction through the shared GROUP tx relay destination.
 * No path lookup required — any reachable node receives and propagates it.
 * @param txBytes  Raw serialised Solana transaction (transaction.serialize())
 */
export async function sendTx(txBytes: Uint8Array): Promise<boolean> {
  return ReticulumNative.sendTx(Array.from(txBytes));
}

/**
 * Returns the deterministic GROUP tx relay hash (32 hex chars).
 * Identical on every anon0mesh node worldwide.
 */
export async function txGroupHash(): Promise<string> {
  return ReticulumNative.txGroupHash();
}

/**
 * Send a direct message to a peer (32 hex char destination hash).
 */
export async function sendTo(destHex: string, payload: Uint8Array): Promise<boolean> {
  return ReticulumNative.sendTo(destHex, Array.from(payload));
}

/** Returns this node's local destination hash (32 hex chars). */
export async function localHash(): Promise<string> {
  return ReticulumNative.localHash();
}

// ── Peer discovery ────────────────────────────────────────────────────────────

export interface Peer {
  /** 32 hex-char Reticulum destination hash. */
  hash: string;
  /**
   * Raw app_data bytes from the announce packet.
   * For anon0mesh nodes this is empty unless the app sets a display name.
   * Convert to string with: new TextDecoder().decode(new Uint8Array(peer.appData))
   */
  appData: number[];
}

/** Returns the number of reachable peers currently in the peer table. */
export function peerCount(): number {
  return ReticulumNative.peerCount();
}

/**
 * Returns a snapshot of all known peers, sorted by hash.
 * Each entry is { hash: string, appData: number[] }.
 */
export function peers(): Peer[] {
  return ReticulumNative.peers();
}

/** Clear the entire peer table. */
export function clearPeers(): void {
  ReticulumNative.clearPeers();
}

export interface StoredMessage {
  timestamp: string;
  src_hash: string;
  dest_hash: string;
  content: string;
}

/**
 * Fetch historical messages from the native SQLite database.
 * @param limit Max number of messages to return.
 * @returns A JSON string (array of StoredMessage) from the native layer.
 */
export async function fetchMessages(limit: number = 50): Promise<StoredMessage[]> {
  const json = await ReticulumNative.fetchMessages(limit);
  return JSON.parse(json);
}

// ── Events ────────────────────────────────────────────────────────────────────

/** Fires when a Solana tx arrives via the GROUP relay. */
export function addTxListener(l: (e: TxReceivedEvent) => void): EventSubscription {
  return ReticulumNative.addListener('onTxReceived', l);
}

/** Fires when a message arrives addressed to our SINGLE destination. */
export function addPacketListener(l: (e: PacketReceivedEvent) => void): EventSubscription {
  return ReticulumNative.addListener('onPacketReceived', l);
}

export function addStateListener(l: (e: MeshStateEvent) => void): EventSubscription {
  return ReticulumNative.addListener('onMeshStateChanged', l);
}

export function addPathListener(l: (e: PathFoundEvent) => void): EventSubscription {
  return ReticulumNative.addListener('onPathFound', l);
}

/**
 * Fires when Rust has a packet ready to transmit over a physical radio.
 * Listen here and write event.data to the BLE characteristic / LoRa port
 * corresponding to event.iface.
 *
 * NOTE: The "auto" interface handles its own UDP I/O in Rust — this event
 * will NOT fire for auto. Only "ble" and "lora" emit outgoing packets.
 */
export function addOutgoingListener(l: (e: OutgoingPacketEvent) => void): EventSubscription {
  return ReticulumNative.addListener('onOutgoingPacket', l);
}
