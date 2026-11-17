const { runTool, waitForPendingCount, waitForResult, waitForStubReady } = require('../support/waits');

describe('denying a tool that asks', () => {
    it('fails the execution instead of invoking the connector', async () => {
        await waitForStubReady();

        await runTool('echo__ask', {});
        await waitForPendingCount(1);

        await $('#deny').click();
        const [result] = await waitForResult();

        expect(result.is_error).toBe(true);
        expect(result.content).toContain('not approved');
        await waitForPendingCount(0);
    });
});
