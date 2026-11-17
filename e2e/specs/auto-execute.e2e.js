const { runTool, textOf, waitForResult, waitForStubReady } = require('../support/waits');

describe('executing an auto-approved tool', () => {
    it('returns the connector result without raising an approval', async () => {
        await waitForStubReady();

        await runTool('echo__auto', {});
        const [result] = await waitForResult();

        expect(result.is_error).toBeFalsy();
        expect(JSON.parse(result.content)).toEqual({ echo: {} });
        expect(await textOf('#pending')).toBe('0');
    });
});
