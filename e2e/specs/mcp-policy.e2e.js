const { runTool, waitForPendingCount, waitForStubReady } = require('../support/waits');
const { mcpId, prepareSecurityTools, disableSecurityTools, executeTool, setPolicy, approvalReason, resolve, marker, callsFor } = require('../support/security');

describe('MCP approval defaults through discovery and the native registry', () => {
    before(async () => prepareSecurityTools());
    after(async () => disableSecurityTools());

    it('asks for every hand-added tool, regardless of its safety hints', async () => {
        const policies = await browser.execute(() => {
            const server = window.LumoConnectors.getManifests().find((item) => item.id === 'mcp-security');
            return Object.fromEntries(server.tools.map((tool) => [tool.name, tool.policy]));
        });
        expect(policies).toEqual({ read: 'ask', outbound: 'ask', write: 'ask', unannotated: 'ask' });
    });

    it('respects auto-approval until a service result reaches the model', async () => {
        await setPolicy(mcpId, 'write', 'auto_approve');
        try {
            const input = { marker: marker() };
            await executeTool(`${mcpId}__write`, input);
            expect(callsFor(input.marker)).toEqual([{ tool: 'write', ...input }]);

            const next = { marker: marker() };
            await runTool(`${mcpId}__write`, next);
            expect(await approvalReason()).toEqual({ kind: 'untrusted_content', sources: ['service', 'connector'] });
            expect(callsFor(next.marker)).toEqual([]);
            expect((await resolve(false)).is_error).toBe(true);
            expect(callsFor(next.marker)).toEqual([]);
        } finally {
            await setPolicy(mcpId, 'write', 'ask');
        }
    });

    for (const tool of ['read', 'outbound', 'write', 'unannotated']) {
        it(`requires approval for ${tool}, with no execution when denied`, async () => {
            const input = { marker: marker() };
            await runTool(`${mcpId}__${tool}`, input);
            await waitForPendingCount(1);
            expect(callsFor(input.marker)).toEqual([]);
            expect((await resolve(false)).is_error).toBe(true);
            expect(callsFor(input.marker)).toEqual([]);

            await runTool(`${mcpId}__${tool}`, input);
            await waitForPendingCount(1);
            expect((await resolve(true)).is_error).toBeFalsy();
            expect(callsFor(input.marker)).toEqual([{ tool, ...input }]);
        });
    }

    it('reclassifies old auto-approved caches before the server can connect', async () => {
        await waitForStubReady();
        const policies = await browser.execute(() => {
            const server = window.LumoConnectors.getManifests().find((item) => item.id === 'mcp-legacy-cache');
            return Object.fromEntries(server.tools.map((tool) => [tool.name, tool.policy]));
        });
        expect(policies).toEqual({ read: 'ask', write: 'ask', unannotated: 'ask' });

        // The cached tool remains usable through discovery even while the peer is offline.
        await browser.execute(async () => {
            await window.LumoConnectors.setConnectorEnabled('mcp-legacy-cache', true);
            await window.LumoConnectors.loadRegistry();
        });
        await runTool('mcp-legacy-cache__write', {});
        await waitForPendingCount(1);
        expect((await resolve(false)).is_error).toBe(true);
    });
});
