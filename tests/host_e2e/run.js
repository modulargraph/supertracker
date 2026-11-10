'use strict';
// End-to-end test of the Rust host: the real interface, in Chromium, against `supertracker --serve`.
//
//   node tests/host_e2e/run.js [path/to/supertracker]
//
// The executable defaults to $SUPERTRACKER_BIN, then $CARGO_TARGET_DIR/debug/supertracker, then
// rust/target/{debug,release}/supertracker. Build it first: cd rust && cargo build -p st-app
// See tests/host_e2e/README.md.
const assert = require('node:assert/strict');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawn } = require('node:child_process');
const { chromium } = require('playwright');

const repository = path.join(__dirname, '../..');
const NOTE = { note: 64, instrument: 0, velocity: 97 };

function executable() {
  const candidates = [process.argv[2], process.env.SUPERTRACKER_BIN,
    process.env.CARGO_TARGET_DIR && path.join(process.env.CARGO_TARGET_DIR, 'debug/supertracker'),
    path.join(repository, 'rust/target/debug/supertracker'), path.join(repository, 'rust/target/release/supertracker')].filter(Boolean);
  const found = candidates.find(candidate => fs.existsSync(candidate));
  if (!found) throw new Error('Build the host first (cd rust && cargo build -p st-app) or pass the path to supertracker. Looked in:\n  ' + candidates.join('\n  '));
  return found;
}

let passed = 0;
function pass(what) { passed++; console.log('PASS ' + what); }

/** Starts the host and resolves with its process and the URL it prints first. */
function startHost(bin, folder, extra = []) {
  const host = spawn(bin, ['--serve', '--ui', path.join(repository, 'Resources/Web'), '--dialog-answers', path.join(folder, 'answers.json'), ...extra], {
    env: { ...process.env, SUPERTRACKER_SUPPORT_DIR: path.join(folder, 'support') }, stdio: ['pipe', 'pipe', 'pipe']
  });
  host.log = '';
  host.stderr.on('data', data => { host.log += data; });
  host.exited = new Promise(resolve => host.on('exit', code => resolve(code)));
  return new Promise((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => reject(new Error('The host printed no URL within 20 s.\n' + host.log)), 20000);
    host.stdout.on('data', data => {
      output += data;
      const line = output.split('\n').find(text => text.startsWith('http://127.0.0.1:'));
      if (line) { clearTimeout(timer); resolve({ host, url: line.trim() }); }
    });
    host.on('exit', code => { clearTimeout(timer); reject(new Error(`The host exited with ${code} before printing its URL.\n` + host.log)); });
  });
}

async function quit(host) {
  host.stdin.write('quit\n');
  const code = await Promise.race([host.exited, new Promise(resolve => setTimeout(() => resolve('timeout'), 10000))]);
  if (code === 'timeout') host.kill('SIGKILL');
  return code;
}

/** Waits until the page has finished its handshake with the host. */
async function booted(page) {
  await page.waitForFunction(() => window.ST?.state?.engineReady === true, null, { timeout: 20000 });
  return page.evaluate(() => ({
    native: ST.state.native, transport: STHostTransport.kind, commands: [...(ST.state.hostCommands || [])],
    badge: document.querySelector('.native-badge')?.textContent || ''
  }));
}

const toasts = page => page.evaluate(() => document.querySelector('#toast-region').innerText);
const noteAt = page => page.evaluate(() => ST.state.project.patterns[0].steps[0][0]);

/** One request through the page's bridge: {ok: result} or {error: message}. */
const request = (page, command, payload = {}) => page.evaluate(([name, data]) =>
  ST.request(name, data).then(result => ({ ok: result }), error => ({ error: error.message })), [command, payload]);

function openSocket(url, options) {
  return new Promise(resolve => {
    const socket = new WebSocket(url, options);
    socket.onopen = () => { socket.close(); resolve('open'); };
    socket.onerror = () => resolve('refused');
  });
}

async function main() {
  const bin = executable();
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'supertracker-e2e-'));
  const saved = path.join(folder, 'E2E Song.supertracker');
  // The file choosers of this run, in order: Save (the project file), then Open (the same file).
  fs.writeFileSync(path.join(folder, 'answers.json'), JSON.stringify([saved, saved]));
  let host, browser;
  try {
    let url;
    ({ host, url } = await startHost(bin, folder));
    assert.match(url, /^http:\/\/127\.0\.0\.1:\d+\/\?token=[0-9a-f]{32}$/);
    pass('the host prints a ready-to-open URL with its access token: ' + url.replace(/token=.*/, 'token=…'));

    // ---- The bridge is closed to everyone but the page the host serves -----------------------
    const origin = new URL(url).origin, token = new URL(url).searchParams.get('token');
    const socketBase = origin.replace('http', 'ws') + '/bridge?token=';
    assert.equal((await fetch(origin + '/index.html')).status, 200);
    assert.equal((await fetch(origin + '/../rust/Cargo.toml')).status, 404);
    assert.equal((await fetch(origin + '/bridge?token=' + token)).status, 426, 'the bridge is a WebSocket, not a page');
    assert.equal(await openSocket(socketBase + 'wrong'), 'refused');
    // Node's WebSocket sends no Origin header: a client that is not the served page is refused even with the token.
    assert.equal(await openSocket(socketBase + token), 'refused');
    pass('files are served from the interface folder only; the bridge refuses a wrong token and a foreign origin');

    browser = await chromium.launch();
    const context = await browser.newContext();
    const page = await context.newPage();
    const problems = [];
    page.on('pageerror', error => problems.push('page error: ' + error.message));
    page.on('console', message => { if (message.type() === 'error') problems.push('console: ' + message.text()); });

    // ---- Boot against the real host -------------------------------------------------------------
    await page.goto(url);
    const boot = await booted(page);
    assert.equal(boot.native, true, 'the page must not be in browser-preview mode');
    assert.equal(boot.transport, 'websocket');
    for (const command of ['ready', 'engine.project', 'project.save', 'project.open', 'settings.get']) assert.ok(boot.commands.includes(command), command);
    assert.ok(!boot.commands.includes('ui.contextMenu'), 'a host without a window has no native menus');
    const ready = await request(page, 'ready');
    assert.equal(ready.ok.version, '0.1.0');
    assert.notEqual(ready.ok.version, 'browser preview');
    assert.deepEqual([ready.ok.roots, ready.ok.plugins, ready.ok.audio, ready.ok.midi], [[], [], { inputs: [], outputs: [] }, { inputs: [], outputs: [] }]);
    pass(`the interface boots against the host over the WebSocket (${boot.commands.length} commands implemented, no preview fallback)`);

    // ---- Create a note, save through the Save window (answered from the answers file) --------
    await page.evaluate(note => ST.mutate('E2E note', () => { ST.state.project.patterns[0].steps[0][0] = note; }), NOTE);
    assert.deepEqual(await noteAt(page), NOTE);
    assert.equal(await page.evaluate(() => ST.state.dirty), true);
    assert.equal(await page.evaluate(() => ST.projectAction('save')), true, 'save failed: ' + await toasts(page));
    assert.equal(await page.evaluate(() => ST.state.dirty), false);
    assert.match(await toasts(page), /Project saved · E2E Song\.supertracker/);
    const onDisk = JSON.parse(fs.readFileSync(saved, 'utf8'));
    assert.deepEqual(onDisk.patterns[0].steps[0][0], NOTE);
    pass('a note entered in the page is saved to the chosen file through project.save');

    // ---- An unimplemented command fails with its name, and the page carries on ---------------
    const refused = await request(page, 'plugin.load', { track: 0, kind: 'instrument' });
    assert.equal(refused.error, '`plugin.load` is not implemented in the Rust host yet (it belongs to st-audio).');
    assert.equal((await request(page, 'no.such.command')).error, '`no.such.command` is not a command this host knows.');
    await page.evaluate(() => ST.perform('play'));
    await page.waitForFunction(() => /`transport\.\w+` is not implemented in the Rust host yet/.test(document.querySelector('#toast-region').innerText), null, { timeout: 5000 });
    assert.deepEqual(await request(page, 'settings.set', { key: 'e2e', value: 7 }), { ok: {} });
    assert.deepEqual(await request(page, 'settings.get', { key: 'e2e' }), { ok: { value: 7 } });
    assert.deepEqual(await noteAt(page), NOTE);
    pass('an unimplemented command shows "not implemented" in the page and the next requests still work');

    // ---- Reload: the host hands the session back; New clears it; Open (through the Open window) restores it
    await page.reload();
    const again = await booted(page);
    assert.equal(again.native, true);
    assert.deepEqual(await noteAt(page), NOTE, 'the reloaded page gets the session the host holds');
    assert.equal(await page.evaluate(() => ST.projectAction('new')), true, 'new failed: ' + await toasts(page));
    assert.equal(await noteAt(page), null, 'a new session has no note');
    assert.equal(await page.evaluate(() => ST.projectAction('open')), true, 'open failed: ' + await toasts(page));
    assert.deepEqual(await noteAt(page), NOTE);
    assert.equal(await page.evaluate(() => ST.state.project.name), onDisk.name);
    assert.equal(await page.evaluate(() => ST.state.dirty), false);
    pass('after a reload and New, opening the saved file through project.open brings the note back');

    // ---- The session survives the host: quit it cleanly, start it again, and the page gets the project
    const foreign = problems.filter(text => !/not implemented in the Rust host yet|Failed to load resource/.test(text));
    assert.deepEqual(foreign, [], 'the page reported errors');
    assert.equal(await quit(host), 0, 'the host must exit cleanly on quit\n' + host.log);
    await page.waitForFunction(() => /connection to the SuperTracker host was lost/.test(document.querySelector('#toast-region').innerText), null, { timeout: 5000 });
    assert.match((await request(page, 'settings.get', {})).error, /connection to the SuperTracker host was lost/);
    pass('quit ends the host with status 0, and the page says the connection was lost instead of waiting');

    ({ host, url } = await startHost(bin, folder));
    await page.goto(url);
    await booted(page);
    assert.deepEqual(await noteAt(page), NOTE);
    assert.deepEqual(await request(page, 'settings.get', { key: 'e2e' }), { ok: { value: 7 } });
    const restored = await request(page, 'ready');
    assert.equal(restored.ok.path, saved);
    assert.equal(restored.ok.dirty, false);
    pass('a new host process restores the session (recovery copy), its file and the settings');
    assert.equal(await quit(host), 0);
    host = null;
    console.log(`\n${passed} checks passed.`);
  } finally {
    await browser?.close();
    if (host && host.exitCode === null) host.kill('SIGKILL');
    fs.rmSync(folder, { recursive: true, force: true });
  }
}

main().catch(error => { console.error('FAIL', error); process.exit(1); });
