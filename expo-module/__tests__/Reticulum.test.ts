import * as Reticulum from '../src/index';
import { requireNativeModule } from 'expo-modules-core';

const mockModule = (requireNativeModule as jest.Mock)();

describe('Reticulum API', () => {
    beforeEach(() => {
        jest.clearAllMocks();
    });

    test('init calls native module', async () => {
        const success = await Reticulum.init('/path/to/identity');
        expect(mockModule.init).toHaveBeenCalledWith('/path/to/identity');
        expect(success).toBe(true);
    });

    test('start calls native module', async () => {
        await Reticulum.start();
        expect(mockModule.start).toHaveBeenCalled();
    });

    test('addInterface handles arguments correctly', () => {
        Reticulum.addInterface('ble');
        expect(mockModule.addInterface).toHaveBeenCalledWith('ble', '');

        Reticulum.addInterface('tcp_client', '1.2.3.4:4242');
        expect(mockModule.addInterface).toHaveBeenCalledWith('tcp_client', '1.2.3.4:4242');
    });

    test('sendTx converts Uint8Array to number array for FFI', async () => {
        const data = new Uint8Array([1, 2, 3]);
        await Reticulum.sendTx(data);
        expect(mockModule.sendTx).toHaveBeenCalledWith([1, 2, 3]);
    });

    test('localHash returns native value', async () => {
        const hash = await Reticulum.localHash();
        expect(hash).toBe('local_hash_hex');
    });
});
