const { waitForStubReady } = require('../support/waits');

// The suite runs inside the e2e container (see run.sh), which deliberately has no
// Secret Service on the session bus (creating one needs a graphical prompter). So the
// expected verdict is "unavailable"
describe('secure storage detection on a machine with no credential store', () => {
    it('reports storage unavailable when no Secret Service is on the session bus', async () => {
        await waitForStubReady();

        await browser.execute(() => {
            window.__secureStorageVerdict = 'pending';
            window.__TAURI_INTERNALS__
                .invoke('is_secure_storage_available')
                .then((available) => {
                    window.__secureStorageVerdict = available ? 'available' : 'unavailable';
                })
                .catch((e) => {
                    window.__secureStorageVerdict = `error: ${e}`;
                });
        });

        await browser.waitUntil(
            async () => (await browser.execute(() => window.__secureStorageVerdict)) !== 'pending',
            { timeout: 20_000, timeoutMsg: 'the storage probe never returned' }
        );

        const verdict = await browser.execute(() => window.__secureStorageVerdict);
        expect(verdict).toBe('unavailable');
    });
});
