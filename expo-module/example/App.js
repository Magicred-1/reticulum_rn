import React, { useEffect, useState } from 'react';
import { 
  StyleSheet, 
  Text, 
  View, 
  ScrollView, 
  TouchableOpacity, 
  SafeAreaView, 
  StatusBar,
  ActivityIndicator,
  FlatList,
  Platform
} from 'react-native';
import * as Notifications from 'expo-notifications';
import { useMesh } from 'expo-reticulum';

export default function App() {
  const { 
    running, 
    localHash, 
    peers, 
    messages, 
    start, 
    stop, 
    refreshPeers, 
    fetchMessages 
  } = useMesh({
    interfaces: ['auto'] // Using auto (UDP) for the demo
  });

  const [loading, setLoading] = useState(false);

  // Request notification permissions for the Foreground Service
  useEffect(() => {
    (async () => {
      if (Platform.OS === 'android') {
        const { status } = await Notifications.requestPermissionsAsync();
        if (status !== 'granted') {
          console.warn('Notification permissions required for background daemon');
        }
      }
    })();
  }, []);

  // Auto-refresh data when running
  useEffect(() => {
    let timer;
    if (running) {
      timer = setInterval(() => {
        refreshPeers();
        fetchMessages(20);
      }, 5000);
    }
    return () => clearInterval(timer);
  }, [running, refreshPeers, fetchMessages]);

  const toggleMesh = async () => {
    setLoading(true);
    try {
      if (running) {
        stop();
      } else {
        await start();
      }
    } catch (err) {
      console.error(err);
    } finally {
      setLoading(false);
    }
  };

  const renderMessage = ({ item }) => (
    <View style={styles.messageCard}>
      <Text style={styles.msgHash}>{item.src_hash.substring(0, 8)}... → {item.dest_hash.substring(0, 8)}...</Text>
      <Text style={styles.msgContent}>{item.content}</Text>
      <Text style={styles.msgTime}>{new Date(item.timestamp).toLocaleTimeString()}</Text>
    </View>
  );

  return (
    <SafeAreaView style={styles.container}>
      <StatusBar barStyle="light-content" />
      
      <View style={styles.header}>
        <Text style={styles.title}>Reticulum Daemon</Text>
        <TouchableOpacity 
          style={[styles.statusBadge, { backgroundColor: running ? '#00E676' : '#FF5252' }]}
          onPress={toggleMesh}
          disabled={loading}
        >
          {loading ? (
            <ActivityIndicator size="small" color="#fff" />
          ) : (
            <Text style={styles.statusText}>{running ? 'RUNNING' : 'STOPPED'}</Text>
          )}
        </TouchableOpacity>
      </View>

      <ScrollView style={styles.content}>
        <View style={styles.card}>
          <Text style={styles.label}>Local Identity Hash</Text>
          <Text style={styles.value}>{localHash || 'Not Initialized'}</Text>
        </View>

        <View style={styles.row}>
          <View style={[styles.card, { flex: 1, marginRight: 8 }]}>
            <Text style={styles.label}>Active Peers</Text>
            <Text style={styles.value}>{peers.length}</Text>
          </View>
          <TouchableOpacity style={[styles.card, { flex: 1 }]} onPress={() => fetchMessages()}>
            <Text style={styles.label}>Messages (DB)</Text>
            <Text style={styles.value}>{messages.length}</Text>
          </TouchableOpacity>
        </View>

        <Text style={styles.sectionTitle}>Identity Registry / Peers</Text>
        {peers.length === 0 ? (
          <Text style={styles.emptyText}>No peers discovered yet...</Text>
        ) : (
          peers.map((peer, i) => (
            <View key={i} style={styles.peerItem}>
              <View style={styles.peerDot} />
              <Text style={styles.peerHash}>{peer.hash}</Text>
            </View>
          ))
        )}

        <Text style={styles.sectionTitle}>Historical Chat (SQLite)</Text>
        <FlatList
          data={messages}
          renderItem={renderMessage}
          keyExtractor={(item, index) => index.toString()}
          scrollEnabled={false}
          ListEmptyComponent={<Text style={styles.emptyText}>Database is empty.</Text>}
        />
      </ScrollView>

      <View style={styles.footer}>
        <Text style={styles.footerText}>
          {running 
            ? 'Daemon is active. You can minimize the app.' 
            : 'Start node to enable background routing.'}
        </Text>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#0A0A0A',
  },
  header: {
    padding: 24,
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  title: {
    fontSize: 28,
    fontWeight: '800',
    color: '#FFFFFF',
    letterSpacing: -0.5,
  },
  statusBadge: {
    paddingHorizontal: 16,
    paddingVertical: 8,
    borderRadius: 20,
    minWidth: 100,
    alignItems: 'center',
  },
  statusText: {
    color: '#000',
    fontSize: 12,
    fontWeight: 'bold',
  },
  content: {
    paddingHorizontal: 16,
  },
  card: {
    backgroundColor: '#1C1C1E',
    borderRadius: 16,
    padding: 16,
    marginBottom: 16,
    borderWidth: 1,
    borderColor: '#2C2C2E',
  },
  row: {
    flexDirection: 'row',
    marginBottom: 8,
  },
  label: {
    color: '#8E8E93',
    fontSize: 12,
    textTransform: 'uppercase',
    fontWeight: '600',
    marginBottom: 4,
  },
  value: {
    color: '#FFFFFF',
    fontSize: 14,
    fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
  },
  sectionTitle: {
    color: '#FFFFFF',
    fontSize: 18,
    fontWeight: '700',
    marginTop: 12,
    marginBottom: 12,
    paddingHorizontal: 4,
  },
  peerItem: {
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: '#1C1C1E',
    padding: 12,
    borderRadius: 12,
    marginBottom: 8,
  },
  peerDot: {
    width: 8,
    height: 8,
    borderRadius: 4,
    backgroundColor: '#00E676',
    marginRight: 12,
  },
  peerHash: {
    color: '#E5E5EA',
    fontSize: 13,
    fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
  },
  messageCard: {
    backgroundColor: '#1C1C1E',
    padding: 12,
    borderRadius: 12,
    marginBottom: 12,
    borderLeftWidth: 3,
    borderLeftColor: '#BB86FC',
  },
  msgHash: {
    color: '#8E8E93',
    fontSize: 10,
    marginBottom: 4,
  },
  msgContent: {
    color: '#FFFFFF',
    fontSize: 15,
    marginBottom: 4,
  },
  msgTime: {
    color: '#48484A',
    fontSize: 10,
    textAlign: 'right',
  },
  emptyText: {
    color: '#48484A',
    fontSize: 14,
    fontStyle: 'italic',
    textAlign: 'center',
    marginTop: 20,
    marginBottom: 40,
  },
  footer: {
    padding: 20,
    borderTopWidth: 1,
    borderTopColor: '#1C1C1E',
  },
  footerText: {
    color: '#8E8E93',
    fontSize: 12,
    textAlign: 'center',
  }
});
