const { assertNoStubError, textOf, waitForStubReady } = require('./waits');

const accountId = 'e2e-test-account';

async function request(operation, input = {}, { error } = {}) {
    await $('#schedule-request').setValue(JSON.stringify({ operation, input }));
    await $('#schedule-run').click();
    await browser.waitUntil(async () => {
        await assertNoStubError();
        return (await textOf('#schedule-result')) !== '';
    }, { timeout: 20_000, timeoutMsg: `schedule ${operation} did not reply` });
    const result = JSON.parse(await textOf('#schedule-result'));
    if (error) {
        expect(result.ok).toBe(false);
        expect(result.error).toContain(error);
    } else if (!result.ok) {
        throw new Error(`schedule ${operation}: ${result.error}`);
    }
    return result.data;
}

async function connect() {
    await waitForStubReady();
    // Exercise the real account-identity stamping used by connector IPC.
    await browser.execute((id) => {
        localStorage.clear();
        localStorage.setItem('ps-0', JSON.stringify({ UserID: id }));
        localStorage.setItem('llid', '0');
    }, accountId);
    return request('connect', { accountId, protocolVersion: 1 });
}

async function queueRun(connectionId, id = 'bridge-e2e') {
    const at = new Date(Date.now() + 2000).toISOString();
    await request('replace', { connectionId, schedules: [{
        id, timingRevision: 2, enabled: true,
        timing: { kind: 'once', at }, missedRunPolicy: 'run_once',
    }] });
    let runs;
    await browser.waitUntil(async () => {
        runs = await request('pending', { connectionId });
        return runs.some((run) => run.scheduleId === id);
    }, { timeout: 20_000, timeoutMsg: 'native scheduler did not queue the one-time run' });
    return runs.find((run) => run.scheduleId === id);
}

async function acceptedRun(connectionId, id) {
    const run = await queueRun(connectionId, id);
    await request('report', { connectionId, runId: run.runId, status: 'accepted' });
    return run;
}

async function createExecutor(runId, autoApprove, connectorIds = ['echo'], id = 'scheduled') {
    await request('createExecutor', { id, schedule: { runId, connectorIds, autoApprove } });
}

async function startExecution(name = 'echo__ask', id = 'scheduled', args = {}) {
    // Do not await the result: Ask calls intentionally wait for a later approve/deny click.
    await $('#schedule-request').setValue(JSON.stringify({ operation: 'execute', input: {
        id, calls: [{ id: 'schedule-call', name, arguments: JSON.stringify(args) }],
    } }));
    await $('#schedule-run').click();
}

async function executionResult() {
    await browser.waitUntil(async () => {
        await assertNoStubError();
        return (await textOf('#schedule-result')) !== '';
    }, { timeout: 20_000, timeoutMsg: 'scheduled tool execution did not finish' });
    const result = JSON.parse(await textOf('#schedule-result'));
    if (!result.ok) throw new Error(result.error);
    return result.data;
}

module.exports = { accountId, request, connect, queueRun, acceptedRun, createExecutor, startExecution, executionResult };
