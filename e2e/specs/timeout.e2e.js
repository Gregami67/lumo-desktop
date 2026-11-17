const { approvalTimeoutMs, runTool, waitForPendingCount, waitForResult, waitForStubReady } = require('../support/waits');

describe('letting an approval time out', () => {
    it('fails the execution and clears the card the webview is showing', async () => {
        await waitForStubReady();

        await runTool('echo__ask', {});
        await waitForPendingCount(1);

        // Never answer. Native times out, denies the dispatch, and emits
        // tool:approval_resolved so the webview drops the card it is still showing.
        const [result] = await waitForResult({ timeout: approvalTimeoutMs + 20_000 });

        expect(result.is_error).toBe(true);
        expect(result.content).toContain('not approved');
        await waitForPendingCount(0);
    });
});
