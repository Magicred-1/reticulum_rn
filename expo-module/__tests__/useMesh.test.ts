import { renderHook, act } from '@testing-library/react-hooks';
import { useMesh } from '../src/useMesh';
import { requireNativeModule } from 'expo-modules-core';

const mockModule = requireNativeModule('Reticulum');
const waitForAsync = () => new Promise(resolve => setTimeout(resolve, 50));

describe('useMesh hook deep testing', () => {
    beforeEach(() => {
        jest.clearAllMocks();
    });

    test('buffers incoming packets in state', async () => {
        const { result } = renderHook(() => useMesh());

        await act(async () => {
            await waitForAsync(); // wait for bootstrap
        });

        // Simulate an incoming packet from native bridge
        act(() => {
            (mockModule as any).emit('onPacketReceived', {
                iface: 'ble',
                data: [0, 104, 105], // Tag 0x00, bytes 'hi'
            });
        });

        expect(result.current.packets).toHaveLength(1);
        expect(result.current.packets[0].iface).toBe('ble');
        expect(result.current.packets[0].data).toEqual(new Uint8Array([0, 104, 105]));
    });

    test('triggers onOutgoing callback when Rust core has data', async () => {
        const onOutgoing = jest.fn();
        renderHook(() => useMesh({ onOutgoing }));

        await act(async () => {
            await waitForAsync();
        });

        act(() => {
            (mockModule as any).emit('onOutgoingPacket', {
                iface: 'lora',
                data: [1, 2, 3],
            });
        });

        expect(onOutgoing).toHaveBeenCalledWith({
            iface: 'lora',
            data: [1, 2, 3],
        });
    });

    test('refreshPeers updates peer state from native module', async () => {
        const mockPeers = [{ hash: 'abc', app_data: [1, 2, 3] }];
        (mockModule.peers as jest.Mock).mockReturnValue(mockPeers);

        const { result } = renderHook(() => useMesh());

        await act(async () => {
            await waitForAsync();
        });

        act(() => {
            result.current.refreshPeers();
        });

        expect(result.current.peers).toEqual(mockPeers);
        expect(mockModule.peers).toHaveBeenCalled();
    });
});
