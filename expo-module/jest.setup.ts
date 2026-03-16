import { EventEmitter } from 'events';

jest.mock('expo-file-system/legacy', () => ({
    documentDirectory: '/mock/documents/',
    makeDirectoryAsync: jest.fn().mockResolvedValue(undefined),
}));

const nativeEmitter = new EventEmitter();

jest.mock('expo-modules-core', () => {
    const mockModule = {
        init: jest.fn().mockResolvedValue(true),
        start: jest.fn().mockResolvedValue(true),
        stop: jest.fn(),
        isRunning: jest.fn().mockReturnValue(false),
        addInterface: jest.fn().mockReturnValue(0),
        pushRx: jest.fn(),
        sendTx: jest.fn().mockResolvedValue(true),
        sendTo: jest.fn().mockResolvedValue(true),
        txGroupHash: jest.fn().mockResolvedValue('group_hash_hex'),
        localHash: jest.fn().mockResolvedValue('local_hash_hex'),
        peerCount: jest.fn().mockReturnValue(0),
        peers: jest.fn().mockReturnValue([]),
        clearPeers: jest.fn(),
        addListener: jest.fn((event, listener) => {
            nativeEmitter.on(event, listener);
            return {
                remove: () => nativeEmitter.off(event, listener),
            };
        }),
        // Helper for tests to emit events
        emit: (event: string, payload: any) => nativeEmitter.emit(event, payload),
    };

    return {
        requireNativeModule: jest.fn().mockReturnValue(mockModule),
    };
});
