const { runTool, textOf, waitForPendingCount, waitForResult, waitForStubReady } = require('../support/waits');

describe('approving a tool that asks', () => {
    it('completes the execution the native core is waiting on', async () => {
        await waitForStubReady();

        await runTool('echo__ask', {});
        await waitForPendingCount(1);
        expect(await textOf('#pending-id')).not.toBe('');

        await $('#approve').click();
        const [result] = await waitForResult();

        expect(result.is_error).toBeFalsy();
        expect(JSON.parse(result.content)).toEqual({ echo: {} });
        await waitForPendingCount(0);
    });
});
