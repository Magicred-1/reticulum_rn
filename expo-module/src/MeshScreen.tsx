/**
 * MeshScreen.tsx — example screen showing full Reticulum integration
 * for anon0mesh. Drop this into your app/ directory.
 */
import React, { useCallback } from 'react';
import { View, Text, Pressable, FlatList, StyleSheet } from 'react-native';
import { useMesh, MeshPacket } from './useMesh';

export default function MeshScreen() {
  const {
    running,
    localHash,
    txGroupHash,
    packets,
    start,
    stop,
    sendTx,
    clearPackets,
    peers,
    refreshPeers,
  } = useMesh({
    interfaces: ['ble', 'lora', 'auto'],
    onTx: (txBytes) => {
      // A Solana durable-nonce tx arrived via the GROUP relay.
      // Hand it to your offline tx queue for submission when
      // internet is next available.
      console.log('[mesh] Solana tx received via group, len:', txBytes.length);
    },
    onPacket: (pkt) => {
      console.log('[mesh] message from', pkt.iface, 'len:', pkt.data.length);
    },
  });

  // Example: serialise and send a pre-built Solana tx through the group
  const handleSendTestTx = useCallback(async () => {
    // In real usage: const txBytes = transaction.serialize()
    const mockTx = new TextEncoder().encode('mock_solana_tx_bytes');
    const ok = await sendTx(mockTx);
    console.log('[mesh] sendTx:', ok ? 'queued' : 'failed');
  }, [sendTx]);


  const renderPacket = ({ item }: { item: MeshPacket }) => (
    <View style={styles.packet}>
      <Text style={styles.packetMeta}>
        {item.iface} · {item.timestamp.toLocaleTimeString()}
      </Text>
      <Text style={styles.packetData} numberOfLines={1}>
        {Array.from(item.data.slice(0, 32))
          .map(b => b.toString(16).padStart(2, '0')).join(' ')}
        {item.data.length > 32 ? '…' : ''}
      </Text>
    </View>
  );

  return (
    <View style={styles.container}>

      <View style={styles.header}>
        <View style={[styles.dot, { backgroundColor: running ? '#1D9E75' : '#888780' }]} />
        <Text style={styles.status}>{running ? 'Mesh running' : 'Offline'}</Text>
      </View>

      {localHash && (
        <View style={styles.hashRow}>
          <Text style={styles.hashLabel}>node  </Text>
          <Text style={styles.hash} numberOfLines={1}>{localHash}</Text>
        </View>
      )}
      {txGroupHash && (
        <View style={styles.hashRow}>
          <Text style={styles.hashLabel}>group </Text>
          <Text style={[styles.hash, styles.hashGroup]} numberOfLines={1}>{txGroupHash}</Text>
        </View>
      )}

      <View style={styles.actions}>
        <Pressable
          style={[styles.btn, running ? styles.btnStop : styles.btnStart]}
          onPress={running ? stop : start}
        >
          <Text style={styles.btnText}>{running ? 'Stop' : 'Start'}</Text>
        </Pressable>

        <Pressable
          style={[styles.btn, styles.btnTx]}
          onPress={handleSendTestTx}
          disabled={!running}
        >
          <Text style={styles.btnText}>Send tx</Text>
        </Pressable>


        <Pressable style={[styles.btn, styles.btnAction]} onPress={clearPackets}>
          <Text style={styles.btnText}>Clear</Text>
        </Pressable>
      </View>


      <View style={styles.peerHeader}>
        <Text style={styles.sectionLabel}>
          Reachable peers ({peers.length})
        </Text>
        <Pressable onPress={refreshPeers} style={styles.refreshBtn}>
          <Text style={styles.btnText}>Refresh</Text>
        </Pressable>
      </View>

      {peers.length === 0 ? (
        <Text style={styles.emptyHint}>No peers yet — waiting for announces...</Text>
      ) : (
        peers.map(p => (
          <View key={p.hash} style={styles.peer}>
            <Text style={styles.peerHash} numberOfLines={1}>{p.hash}</Text>
            {p.appData.length > 0 && (
              <Text style={styles.peerMeta}>
                {new TextDecoder().decode(new Uint8Array(p.appData))}
              </Text>
            )}
          </View>
        ))
      )}

      <Text style={[styles.sectionLabel, { marginTop: 16 }]}>
        Received packets ({packets.length})
      </Text>

      <FlatList
        data={packets}
        keyExtractor={p => p.id}
        renderItem={renderPacket}
        style={styles.list}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1, backgroundColor: '#0e0e0e', padding: 16 },
  header: { flexDirection: 'row', alignItems: 'center', marginBottom: 8 },
  dot: { width: 10, height: 10, borderRadius: 5, marginRight: 8 },
  status: { color: '#c2c0b6', fontSize: 16, fontWeight: '500' },
  hashRow: { flexDirection: 'row', alignItems: 'center', marginBottom: 3 },
  hashLabel: { color: '#5f5e5a', fontFamily: 'monospace', fontSize: 11, width: 44 },
  hash: { color: '#5DCAA5', fontFamily: 'monospace', fontSize: 11, flex: 1 },
  hashGroup: { color: '#7F77DD' },  // purple = GROUP dest
  actions: { flexDirection: 'row', gap: 8, marginTop: 14, marginBottom: 20 },
  btn: { paddingHorizontal: 14, paddingVertical: 8, borderRadius: 8 },
  btnStart: { backgroundColor: '#0F6E56' },
  btnStop: { backgroundColor: '#993C1D' },
  btnTx: { backgroundColor: '#0F6E56' },  // teal = Solana tx action
  btnAction: { backgroundColor: '#3C3489' },
  btnText: { color: '#fff', fontSize: 13, fontWeight: '500' },
  sectionLabel: { color: '#888780', fontSize: 12, marginBottom: 8 },
  list: { flex: 1 },
  packet: { backgroundColor: '#1a1a1a', borderRadius: 8, padding: 10, marginBottom: 6 },
  packetMeta: { color: '#5f5e5a', fontSize: 11, marginBottom: 2 },
  packetData: { color: '#c2c0b6', fontFamily: 'monospace', fontSize: 11 },
  peerHeader: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', marginBottom: 6 },
  refreshBtn: { backgroundColor: '#3C3489', paddingHorizontal: 10, paddingVertical: 5, borderRadius: 6 },
  emptyHint: { color: '#5f5e5a', fontSize: 12, marginBottom: 12, fontStyle: 'italic' },
  peer: { backgroundColor: '#161622', borderRadius: 8, padding: 8, marginBottom: 5 },
  peerHash: { color: '#5DCAA5', fontFamily: 'monospace', fontSize: 11 },
  peerMeta: { color: '#888780', fontSize: 11, marginTop: 2 },
});
