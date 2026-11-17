const { waitForStubReady } = require('../support/waits');

// Which Proton account the app is acting as is read out of the web app's own localStorage, whose
// layout is private API in the clients repo. These fixtures are the guard against it drifting.
const cases = [
    {
        name: 'the path wins over the last-used slot',
        pathname: '/u/1/chat',
        store: { 'ps-0': { UserID: 'AAAA' }, 'ps-1': { UserID: 'BBBB' }, llid: '0' },
        expected: { userId: 'BBBB', signal: 'path' },
    },
    {
        name: 'the last-used slot is used when the path names none',
        pathname: '/',
        store: { 'ps-0': { UserID: 'AAAA' }, 'ps-1': { UserID: 'BBBB' }, llid: '1' },
        expected: { userId: 'BBBB', signal: 'llid' },
    },
    {
        name: 'a single stored session needs no other signal',
        pathname: '/',
        store: { 'ps-1': { UserID: 'BBBB' } },
        expected: { userId: 'BBBB', signal: 'single-session' },
    },
    {
        // `llid` is not cleared on sign-out, so it routinely points at an emptied slot. Falling
        // through to the remaining session would hand over another account's secrets.
        name: 'a last-used slot with no session does not fall through to another account',
        pathname: '/',
        store: { 'ps-1': { UserID: 'BBBB' }, llid: '0' },
        expected: { userId: null, signal: 'llid' },
    },
    {
        name: 'two stored sessions with no other signal is ambiguous, so no account',
        pathname: '/',
        store: { 'ps-0': { UserID: 'AAAA' }, 'ps-1': { UserID: 'BBBB' } },
        expected: { userId: null, signal: 'none' },
    },
    {
        name: 'a session carrying no UserID is no account',
        pathname: '/u/0/',
        store: { 'ps-0': { UserID: '' } },
        expected: { userId: null, signal: 'path' },
    },
    {
        name: 'signed out is no account',
        pathname: '/',
        store: {},
        expected: { userId: null, signal: 'none' },
    },
];

describe('resolving which Proton account the web app is signed in as', () => {
    it('reads the account from the stored sessions, and refuses to guess', async () => {
        await waitForStubReady();

        for (const { name, pathname, store, expected } of cases) {
            const resolved = await browser.execute(
                (seed, path) => {
                    localStorage.clear();
                    for (const [key, value] of Object.entries(seed)) {
                        localStorage.setItem(key, typeof value === 'string' ? value : JSON.stringify(value));
                    }
                    return window.__lumoAccountIdentity.resolve(path);
                },
                store,
                pathname
            );

            expect({ case: name, userId: resolved.userId, signal: resolved.signal }).toEqual({
                case: name,
                ...expected,
            });
        }

        await browser.execute(() => localStorage.clear());
    });
});
