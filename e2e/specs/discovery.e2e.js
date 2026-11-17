const { waitForStubReady, textOf } = require('../support/waits');

describe('tool discovery over the bridge', () => {
    it('exposes the echo connector tools to the real web client module', async () => {
        await waitForStubReady();

        const tools = (await textOf('#tools')).split(',').filter(Boolean);

        expect(tools).toContain('echo__auto');
        expect(tools).toContain('echo__ask');
    });
});
