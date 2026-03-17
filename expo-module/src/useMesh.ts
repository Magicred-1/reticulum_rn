import { useEffect, useRef, useState, useCallback } from 'react';
import * as FileSystem from 'expo-file-system/legacy';
import * as Reticulum from './index';
import type { OutgoingPacketEvent, Peer, StoredMessage } from './index';

// ── Types ─────────────────────────────────────────────────────────────────────

export interface MeshPacket {
  id: string;
  iface: string;
  data: Uint8Array;
  timestamp: Date;
}

export type AllowedInterface = 'ble' | 'lora' | 'auto' | 'tcp_client' | 'tcp_server';

export type InterfaceConfig = AllowedInterface | { name: AllowedInterface; arg: string };

export interface UseMeshOptions {
  /** interfaces to enable on mount. e.g. ['ble', 'auto', { name: 'tcp_client', arg: '1.2.3.4:4242' }] */
  interfaces?: InterfaceConfig[];
  onPacket?: (pkt: MeshPacket) => void;
  /** Fires when a Solana tx arrives via the GROUP relay. */
  onTx?: (txBytes: Uint8Array) => void;
  /**
   * Fires when the Rust layer has bytes ready for a physical radio.
   * Write `data` to the BLE characteristic / LoRa serial port for `iface`.
   */
  onOutgoing?: (event: OutgoingPacketEvent) => void;
}

export interface UseMeshReturn {
  running: boolean;
  localHash: string | null;
  txGroupHash: string | null;
  packets: MeshPacket[];
  start: () => Promise<void>;
  stop: () => void;
  sendTx: (txBytes: Uint8Array) => Promise<boolean>;
  sendTo: (destHex: string, data: Uint8Array) => Promise<boolean>;
  /** Push bytes received from a native radio into the named interface. */
  pushRx: (iface: string, data: Uint8Array) => void;
  clearPackets: () => void;
  peers: Peer[];
  refreshPeers: () => void;
  clearPeers: () => void;
  messages: StoredMessage[];
  fetchMessages: (limit?: number) => Promise<void>;
}

// ── Hook ──────────────────────────────────────────────────────────────────────

export function useMesh(options: UseMeshOptions = {}): UseMeshReturn {
  const { interfaces = ['ble', 'lora'], onPacket, onTx, onOutgoing } = options;

  const [running, setRunning] = useState(false);
  const [localHash, setLocalHash] = useState<string | null>(null);
  const [txGroupHash, setTxGroupHash] = useState<string | null>(null);
  const [packets, setPackets] = useState<MeshPacket[]>([]);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [messages, setMessages] = useState<StoredMessage[]>([]);

  // Stable refs so callbacks don't cause subscription re-runs
  const onPacketRef = useRef(onPacket); onPacketRef.current = onPacket;
  const onTxRef = useRef(onTx); onTxRef.current = onTx;
  const onOutgoingRef = useRef(onOutgoing); onOutgoingRef.current = onOutgoing;

  // ── Initialise on mount ─────────────────────────────────────────────────

  useEffect(() => {
    let mounted = true;

    async function bootstrap() {
      const identityDir = `${FileSystem.documentDirectory}reticulum/`;
      const identityPath = `${identityDir}identity`;

      await FileSystem.makeDirectoryAsync(identityDir, { intermediates: true })
        .catch(() => { });

      const ok = await Reticulum.init(identityPath);
      if (!ok || !mounted) return;

      for (const iface of interfaces) {
        let name: AllowedInterface;
        let arg = '';
        if (typeof iface === 'string') {
          name = iface;
        } else {
          name = iface.name;
          arg = iface.arg;
        }
        const idx = Reticulum.addInterface(name, arg);
        if (idx < 0) console.warn(`[mesh] addInterface("${name}") failed`);
      }
    }

    bootstrap();
    return () => { mounted = false; };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Event listeners ─────────────────────────────────────────────────────

  useEffect(() => {
    const packetSub = Reticulum.addPacketListener((event) => {
      const pkt: MeshPacket = {
        id: `${Date.now()}-${Math.random()}`,
        iface: event.iface,
        data: new Uint8Array(event.data),
        timestamp: new Date(),
      };
      setPackets(prev => [pkt, ...prev].slice(0, 200));
      onPacketRef.current?.(pkt);
    });

    const txSub = Reticulum.addTxListener((event) => {
      onTxRef.current?.(new Uint8Array(event.data));
    });

    const stateSub = Reticulum.addStateListener((event) => {
      setRunning(event.running);
    });

    // Outgoing packets — bytes Rust wants to transmit over the physical radio
    const outgoingSub = Reticulum.addOutgoingListener((event) => {
      onOutgoingRef.current?.(event);
    });

    return () => {
      packetSub.remove();
      txSub.remove();
      stateSub.remove();
      outgoingSub.remove();
    };
  }, []);

  // ── Actions ──────────────────────────────────────────────────────────────

  const start = useCallback(async () => {
    const ok = await Reticulum.start();
    if (ok) {
      const [hash, groupHash] = await Promise.all([
        Reticulum.localHash().catch(() => null),
        Reticulum.txGroupHash().catch(() => null),
      ]);
      setLocalHash(hash);
      setTxGroupHash(groupHash);
      // Initial peer list is empty; it populates as announces arrive.
      // Call refreshPeers() on a timer or after user action if needed.
      setPeers([]);
    }
  }, []);

  const stop = useCallback(() => { Reticulum.stop(); }, []);

  const sendTx = useCallback((txBytes: Uint8Array) => {
    return Reticulum.sendTx(txBytes);
  }, []);

  const sendTo = useCallback((destHex: string, data: Uint8Array) => {
    return Reticulum.sendTo(destHex, data);
  }, []);

  const pushRx = useCallback((iface: string, data: Uint8Array) => {
    Reticulum.pushRx(iface, data);
  }, []);

  const refreshPeers = useCallback(() => setPeers(Reticulum.peers()), []);
  const clearPeers = useCallback(() => { Reticulum.clearPeers(); setPeers([]); }, []);

  const clearPackets = useCallback(() => setPackets([]), []);

  const fetchMessages = useCallback(async (limit: number = 50) => {
    try {
      const msgs = await Reticulum.fetchMessages(limit);
      setMessages(msgs);
    } catch (err) {
      console.warn('[mesh] fetchMessages error:', err);
    }
  }, []);

  // ── Cleanup ───────────────────────────────────────────────────────────────

  useEffect(() => { return () => { Reticulum.stop(); }; }, []);

  return {
    running, localHash, txGroupHash, packets, peers, messages,
    start, stop, sendTx, sendTo, pushRx, clearPackets, refreshPeers, clearPeers, fetchMessages
  };
}
