const { runTool, textOf, waitForPendingCount, waitForResult } = require('../support/waits');
const { request, connect, queueRun, acceptedRun, createExecutor, startExecution, executionResult } = require('../support/schedules');

async function setPolicy(policy) {
    await browser.execute(async (value) => window.LumoConnectors.setToolPolicy('echo', 'ask', value), policy);
}

async function settle(connectionId, runId) {
    await request('report', { connectionId, runId, status: 'succeeded' });
}

describe('scheduled tool approvals through the real client adapter and native registry', () => {
    afterEach(async () => {
        await setPolicy('ask');
        await browser.execute(() => localStorage.clear());
    });

    it('auto-approves repeated calls without changing approval for ordinary chat', async () => {
        const { connectionId } = await connect();
        const run = await acceptedRun(connectionId, 'approval-auto');
        await createExecutor(run.runId, true);
        for (let i = 0; i < 2; i += 1) {
            await startExecution();
            const [result] = await executionResult();
            expect(result.is_error).toBeFalsy();
            expect(JSON.parse(result.content)).toEqual({ echo: {} });
            expect(await textOf('#pending')).toBe('0');
        }
        await runTool('echo__ask', {});
        await waitForPendingCount(1);
        await $('#approve').click();
        expect((await waitForResult())[0].is_error).toBeFalsy();
        await settle(connectionId, run.runId);
    });

    it('honors manual approval when the schedule toggle is off', async () => {
        const { connectionId } = await connect();
        const run = await acceptedRun(connectionId, 'approval-manual');
        await createExecutor(run.runId, false);
        await startExecution();
        await waitForPendingCount(1);
        expect(await textOf('#schedule-result')).toBe('');
        await $('#approve').click();
        expect((await executionResult())[0].is_error).toBeFalsy();
        await waitForPendingCount(0);
        await settle(connectionId, run.runId);
    });

    it('keeps desktop Disable authoritative even after registering an auto-approved run', async () => {
        const { connectionId } = await connect();
        const run = await acceptedRun(connectionId, 'approval-disabled');
        await createExecutor(run.runId, true);
        await setPolicy('disable');
        await startExecution();
        const [result] = await executionResult();
        expect(result.is_error).toBe(true);
        expect(await textOf('#pending')).toBe('0');
        await settle(connectionId, run.runId);
    });

    it('rejects tools from connectors outside the registered allowlist', async () => {
        const { connectionId } = await connect();
        const run = await acceptedRun(connectionId, 'approval-allowlist');
        await createExecutor(run.runId, true, []);
        await startExecution();
        const [result] = await executionResult();
        expect(result.is_error).toBe(true);
        expect(result.content).toContain('not attached to this scheduled task');
        expect(await textOf('#pending')).toBe('0');
        await settle(connectionId, run.runId);
    });

    it('refuses registration before acceptance and rejects completed execution contexts', async () => {
        const { connectionId } = await connect();
        const run = await queueRun(connectionId, 'approval-lifecycle');
        await request('createExecutor', { id: 'scheduled', schedule: {
            runId: run.runId, connectorIds: ['echo'], autoApprove: true,
        } }, { error: 'not accepted or running' });
        await request('report', { connectionId, runId: run.runId, status: 'accepted' });
        await createExecutor(run.runId, true);
        await settle(connectionId, run.runId);
        await startExecution();
        const [result] = await executionResult();
        expect(result.is_error).toBe(true);
        expect(result.content).toContain('not accepted or running');
    });

    it('fails explicitly when connected to a desktop bridge without scheduled approval support', async () => {
        await connect();
        // Simulate the old bridge's public surface while keeping the real client adapter.
        await browser.execute(() => {
            window.__savedRegisterScheduleExecutionContext = window.__TAURI__.external_tools.registerScheduleExecutionContext;
            delete window.__TAURI__.external_tools.registerScheduleExecutionContext;
        });
        try {
            await request('createExecutor', { id: 'unsupported', schedule: {
                runId: 'not-an-active-run', connectorIds: ['echo'], autoApprove: true,
            } }, { error: 'does not support scheduled tool approval contexts' });
            expect(await textOf('#pending')).toBe('0');
        } finally {
            await browser.execute(() => {
                window.__TAURI__.external_tools.registerScheduleExecutionContext = window.__savedRegisterScheduleExecutionContext;
                delete window.__savedRegisterScheduleExecutionContext;
            });
        }
    });

    it('expires registered permissions when the web client reconnects', async () => {
        const { connectionId } = await connect();
        const run = await acceptedRun(connectionId, 'approval-reconnect');
        await createExecutor(run.runId, true);
        const next = await connect();
        await startExecution();
        const [result] = await executionResult();
        expect(result.is_error).toBe(true);
        expect(result.content).toContain('connection expired');
        await settle(next.connectionId, run.runId);
    });
});
