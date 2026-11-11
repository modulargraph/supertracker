'use strict';
// End-to-end test of the host: the real interface, in Chromium, against `supertracker --serve`.
//
//   node tests/host_e2e/run.js [path/to/supertracker]
//
// The executable defaults to $SUPERTRACKER_BIN, then $CARGO_TARGET_DIR/debug/supertracker, then
// target/{debug,release}/supertracker. Build it first: cargo build -p st-app
// See tests/host_e2e/README.md.
const assert = require('node:assert/strict');
const fs = require('node:fs'), os = require('node:os'), path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const { chromium } = require('playwright');

const repository = path.join(__dirname, '../..');
const NOTE = { note: 64, instrument: 0, velocity: 97 };

function executable() {
  const candidates = [process.argv[2], process.env.SUPERTRACKER_BIN,
    process.env.CARGO_TARGET_DIR && path.join(process.env.CARGO_TARGET_DIR, 'debug/supertracker'),
    path.join(repository, 'target/debug/supertracker'), path.join(repository, 'target/release/supertracker')].filter(Boolean);
  const found = candidates.find(candidate => fs.existsSync(candidate));
  if (!found) throw new Error('Build the host first (cargo build -p st-app) or pass the path to supertracker. Looked in:\n  ' + candidates.join('\n  '));
  return found;
}

let passed = 0;
function pass(what) { passed++; console.log('PASS ' + what); }

/** The environment of every process of a run: its own support folder (and with it its own control socket),
    the null audio device and no MIDI devices, so the run is the same on every machine. */
const environment = folder => ({ ...process.env, SUPERTRACKER_SUPPORT_DIR: path.join(folder, 'support'), SUPERTRACKER_AUDIO_DEVICE: 'null',
  SUPERTRACKER_MIDI: 'off' });

/** Starts the host and resolves with its process and the URL it prints first. */
function startHost(bin, folder, extra = []) {
  const host = spawn(bin, ['--serve', '--ui', path.join(repository, 'ui'), '--dialog-answers', path.join(folder, 'answers.json'), ...extra], {
    env: environment(folder), stdio: ['pipe', 'pipe', 'pipe']
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

/** A short 16-bit mono WAV file. */
function writeWav(file, hertz, frames = 4410) {
  const data = Buffer.alloc(frames * 2), header = Buffer.alloc(44);
  for (let frame = 0; frame < frames; frame++) data.writeInt16LE(Math.round(Math.sin(frame * hertz * 2 * Math.PI / 44100) * 12000), frame * 2);
  header.write('RIFF', 0); header.writeUInt32LE(36 + data.length, 4); header.write('WAVEfmt ', 8); header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20); header.writeUInt16LE(1, 22); header.writeUInt32LE(44100, 24); header.writeUInt32LE(88200, 28);
  header.writeUInt16LE(2, 32); header.writeUInt16LE(16, 34); header.write('data', 36); header.writeUInt32LE(data.length, 40);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, Buffer.concat([header, data]));
}

/** The names shown in the Browser's result list, sorted. */
const browserResults = page => page.evaluate(() => [...document.querySelectorAll('#browser-results-list .browser-result strong')].map(node => node.textContent).sort());

/** Opens the Browser workspace, searches for `query` and waits until the list shows exactly `names`. */
async function searchBrowser(page, query, names) {
  await page.click('#view-browser');
  await page.waitForSelector('#browser-search');
  await page.fill('#browser-search', query);
  await page.waitForFunction(expected => [...document.querySelectorAll('#browser-results-list .browser-result strong')].map(node => node.textContent).sort().join('|') === expected,
    names.join('|'), { timeout: 20000 }).catch(async error => { throw new Error(`The Browser shows ${JSON.stringify(await browserResults(page))}, not ${JSON.stringify(names)}. ${await toasts(page)}\n${error.message}`); });
}

/** `supertracker --ctl <method> [params]` against the host of this run: {status, stdout, stderr}. */
function ctl(bin, folder, method, params) {
  return spawnSync(bin, ['--ctl', method, ...(params ? [JSON.stringify(params)] : [])], { env: environment(folder), encoding: 'utf8', timeout: 30000 });
}

/** An MCP client session with `supertracker --mcp` over its standard input and output. */
function mcpSession(bin, folder) {
  const child = spawn(bin, ['--mcp'], { env: environment(folder), stdio: ['pipe', 'pipe', 'pipe'] });
  const waiting = new Map();
  let buffer = '', next = 1;
  child.stdout.on('data', data => {
    buffer += data;
    for (let end = buffer.indexOf('\n'); end >= 0; end = buffer.indexOf('\n')) {
      const line = buffer.slice(0, end).trim(); buffer = buffer.slice(end + 1);
      if (!line) continue;
      const message = JSON.parse(line);
      waiting.get(message.id)?.(message); waiting.delete(message.id);
    }
  });
  const send = message => child.stdin.write(JSON.stringify(message) + '\n');
  const request = (method, params = {}) => new Promise((resolve, reject) => {
    const id = next++;
    const timer = setTimeout(() => reject(new Error(`The MCP server did not answer ${method} within 30 s.`)), 30000);
    waiting.set(id, message => { clearTimeout(timer); resolve(message); });
    send({ jsonrpc: '2.0', id, method, params });
  });
  return {
    request, notify: method => send({ jsonrpc: '2.0', method }),
    /** Calls a tool: {text, isError}. */
    tool: async (name, args = {}) => { const { result } = await request('tools/call', { name, arguments: args }); return { text: result.content[0].text, isError: result.isError === true }; },
    /** Ends the session: the server exits when its input ends (it is killed if it has not after five seconds). */
    close: () => new Promise(resolve => {
      const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
      child.on('exit', code => { clearTimeout(timer); resolve(code); });
      child.stdin.end();
    })
  };
}

const GAIN = 'clap:org.supertracker.test.gain';

/** The plug-in container and the CLAP test plug-in of crates/plugin-host next to the host executable (the place the
    host looks for its container), built there when missing; the plug-in is installed into `<folder>/Plug-Ins`. */
function installTestPlugin(bin, folder) {
  const build = path.dirname(bin), library = path.join(build, process.platform === 'darwin' ? 'libstpc_clap_test_plugin.dylib' : 'libstpc_clap_test_plugin.so');
  if (!fs.existsSync(path.join(build, 'supertracker-plugin-container')) || !fs.existsSync(library)) {
    const built = spawnSync('cargo', ['build', '--manifest-path', path.join(repository, 'crates/plugin-host/Cargo.toml'), '-p', 'stpc-container', '-p', 'stpc-clap-test-plugin',
      ...(path.basename(build) === 'release' ? ['--release'] : [])], { env: { ...process.env, CARGO_TARGET_DIR: path.dirname(build) }, encoding: 'utf8', timeout: 900000 });
    if (built.status !== 0 || !fs.existsSync(library)) throw new Error('The plug-in container and its test plug-in could not be built next to ' + bin + ':\n' + built.stderr);
  }
  const plugins = path.join(folder, 'Plug-Ins');
  fs.mkdirSync(plugins, { recursive: true });
  if (process.platform === 'darwin') {
    fs.mkdirSync(path.join(plugins, 'ST Test.clap/Contents/MacOS'), { recursive: true });
    fs.copyFileSync(library, path.join(plugins, 'ST Test.clap/Contents/MacOS/ST Test'));
  } else fs.copyFileSync(library, path.join(plugins, 'ST Test.clap'));
  return { plugins, file: path.join(plugins, 'ST Test.clap') };
}

/** The largest value `read` (evaluated in the page) returns while sampling for `milliseconds`. */
async function loudest(page, read, argument, milliseconds = 1500) {
  let peak = 0;
  for (const end = Date.now() + milliseconds; Date.now() < end; await page.waitForTimeout(40)) peak = Math.max(peak, await page.evaluate(read, argument));
  return peak;
}

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
  // A folder of sounds to add to the library from the Browser.
  const sounds = path.join(folder, 'Sounds');
  for (const [name, hertz] of [['Drums/Kick 01', 55], ['Drums/Kick 02', 58], ['Drums/Kick 03', 61], ['Drums/Snare 01', 180], ['Drums/Snare 02', 190]]) writeWav(path.join(sounds, name + '.wav'), hertz);
  // The file choosers of this run, in order: Save (the project file), Open (the same file), then Add folder (the sounds).
  fs.writeFileSync(path.join(folder, 'answers.json'), JSON.stringify([saved, saved, sounds]));
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
    assert.equal((await fetch(origin + '/../Cargo.toml')).status, 404);
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
    assert.match(ready.ok.version, /^\d+\.\d+\.\d+$/);
    assert.notEqual(ready.ok.version, 'browser preview');
    // The library starts with this run's own kit folder; MIDI devices are off for the run.
    assert.deepEqual(ready.ok.roots.map(root => root.path), [path.join(folder, 'support/Kits')]);
    assert.ok(Array.isArray(ready.ok.plugins));
    assert.deepEqual([ready.ok.midi.inputs, ready.ok.midi.outputs], [[], []]);
    assert.match(ready.ok.midi.error, /turned off for this run/);
    for (const command of ['library.search', 'library.scan', 'kit.save', 'midi.devices', 'midi.configure', 'mk3.status', 'agent.status', 'agent.reply']) assert.ok(boot.commands.includes(command), command);
    assert.equal((await request(page, 'mk3.status')).ok.enabled, false, 'Maschine MK3 support is off unless the user turns it on');
    // The audio engine is part of the host: it runs on the null device here.
    assert.equal(ready.ok.audio.outputUid, 'null');
    assert.equal(ready.ok.audio.running, true);
    assert.ok(ready.ok.audio.outputs.some(device => device.uid === 'null' && device.id === 0), 'the null device is listed');
    for (const command of ['transport.play', 'transport.stop', 'note.on', 'audio.devices']) assert.ok(boot.commands.includes(command), command);
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

    // ---- An unknown command fails with its name, and the page carries on ----------------------
    assert.equal((await request(page, 'no.such.command')).error, '`no.such.command` is not a command this host knows.');
    assert.deepEqual(await request(page, 'settings.set', { key: 'e2e', value: 7 }), { ok: {} });
    assert.deepEqual(await request(page, 'settings.get', { key: 'e2e' }), { ok: { value: 7 } });
    assert.deepEqual(await noteAt(page), NOTE);
    pass('an unknown command is refused with its name, and the next requests still work');

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

    // ---- Library: a folder added from the Browser is scanned, searched and shown; a favorite is remembered
    await page.click('#view-browser');
    await page.waitForSelector('#browser-add-root');
    await page.click('#browser-add-root');   // library.chooseRoot: the (scripted) folder chooser, then the scan
    await page.waitForFunction(root => ST.state.library.roots.some(item => item.path === root), sounds, { timeout: 10000 })
      .catch(async error => { throw new Error('The folder was not added: ' + await toasts(page) + '\n' + error.message); });
    await searchBrowser(page, 'kick', ['Kick 01', 'Kick 02', 'Kick 03']);
    await searchBrowser(page, 'snare', ['Snare 01', 'Snare 02']);
    await searchBrowser(page, '', ['Kick 01', 'Kick 02', 'Kick 03', 'Snare 01', 'Snare 02']);
    assert.equal(await page.evaluate(() => ST.state.library.total), 5);
    const scannedRoot = (await request(page, 'library.roots')).ok.find(root => root.path === sounds);
    assert.deepEqual([scannedRoot.available, scannedRoot.indexedCount, scannedRoot.scanStatus], [true, 5, 'complete']);
    await searchBrowser(page, 'kick', ['Kick 01', 'Kick 02', 'Kick 03']);
    await page.click('#browser-results-list .browser-result:has(strong:text-is("Kick 02")) [data-browser-star]');
    await page.waitForSelector('#browser-results-list .browser-result:has(strong:text-is("Kick 02")) .browser-star.is-favorite');
    const favorites = await request(page, 'library.search', { favorites: true });
    assert.deepEqual(favorites.ok.items.map(item => item.name), ['Kick 02']);
    assert.ok(fs.existsSync(path.join(folder, 'support/Library/favorites.json')), 'the library keeps its files in Library/ of the support folder');
    pass('a folder added from the Browser is scanned; searches show its sounds in the result list; a favorite is set');
    await page.click('#view-pattern');

    // ---- The session survives the host: quit it cleanly, start it again, and the page gets the project
    const foreign = problems.filter(text => !/not implemented yet|Failed to load resource/.test(text));
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
    // The library is back from its index cache, and so is the favorite.
    await searchBrowser(page, 'kick', ['Kick 01', 'Kick 02', 'Kick 03']);
    await page.waitForSelector('#browser-results-list .browser-result:has(strong:text-is("Kick 02")) .browser-star.is-favorite');
    assert.equal(await page.locator('#browser-results-list .browser-star.is-favorite').count(), 1);
    assert.ok(restored.ok.roots.some(root => root.path === sounds), 'the added folder is still a library root');
    pass('after the restart the Browser finds the sounds again (index cache) and the favorite is still set');
    await page.click('#view-pattern');
    // ---- Audio: the demo song plays on the null device; the page follows real status events -------
    assert.equal(await page.evaluate(() => ST.projectAction('demo')), true, 'loading the demo failed: ' + await toasts(page));
    assert.equal(await page.evaluate(() => ST.state.project.name), 'Midnight transmission');
    await page.evaluate(() => ST.perform('play'));
    await page.waitForFunction(() => ST.state.playing && ST.state.status.playing === true && ST.state.status.row >= 2
      && ST.state.status.peakL > 0.001 && ST.state.status.peakR > 0.001, null, { timeout: 15000 });
    const heard = await page.evaluate(() => ({ ...ST.state.status }));
    assert.equal(heard.running, true);
    assert.equal(heard.sampleRate, 48000);
    assert.ok(heard.trackPeaks.length === 8 && heard.trackPeaks.some(peak => peak > 0.001), 'track meters move: ' + JSON.stringify(heard.trackPeaks));
    await page.waitForFunction(seconds => ST.state.status.seconds > seconds + 0.25, heard.seconds, { timeout: 10000 });
    assert.doesNotMatch(await toasts(page), /`(transport|engine|note|audio|midi|library|mk3|agent)\.\w+` is not implemented/, 'Play must not answer "not implemented"');
    pass(`Play starts the demo song: the position advances (row ${heard.row}, ${heard.seconds.toFixed(2)} s) and the host reports master peaks ${heard.peakL.toFixed(3)} / ${heard.peakR.toFixed(3)} on the null device`);

    // Editing a note while the song plays is published to the engine without stopping it.
    const LIVE = { note: 45, instrument: 0, velocity: 101, gate: 0.5 };
    const toastsBefore = (await toasts(page)).split('\n');
    await page.evaluate(note => ST.mutate('E2E live edit', () => { ST.state.project.patterns[0].steps[0][2] = note; }), LIVE);
    const deadline = Date.now() + 10000;
    let engineNote = null;
    while (Date.now() < deadline && !engineNote) {
      const snapshot = await request(page, 'engine.snapshot');
      engineNote = snapshot.ok?.patterns?.[0]?.steps?.[0]?.[2] || null;
      if (!engineNote) await page.waitForTimeout(100);
    }
    assert.deepEqual(engineNote, LIVE, 'the edit reaches the engine');
    const before = await page.evaluate(() => ST.state.status.seconds);
    await page.waitForFunction(seconds => ST.state.playing && ST.state.status.seconds !== seconds, before, { timeout: 5000 });
    assert.equal(await page.evaluate(() => ST.state.playing), true, 'playback continues through the edit');
    const newToasts = (await toasts(page)).split('\n').filter(line => line.trim() && !toastsBefore.includes(line));
    assert.deepEqual(newToasts.filter(line => /error|fail|not implemented|could not/i.test(line)), [], 'the edit raised no error');
    pass('a note edited while playing reaches the engine and playback continues');

    await page.evaluate(() => ST.perform('stop'));
    await page.waitForFunction(() => !ST.state.playing && ST.state.status.playing === false, null, { timeout: 5000 });
    const stopped = await page.evaluate(() => ST.state.status.seconds);
    await page.waitForTimeout(300);
    assert.equal(await page.evaluate(() => ST.state.status.seconds), stopped, 'a stopped transport does not move');
    assert.deepEqual(problems.filter(text => !/not implemented yet|Failed to load resource/.test(text)), [], 'the page reported errors while playing');
    pass('Stop stops the transport');

    // ---- Samples and export: a WAV loaded from the page is heard; the song is exported to a file -------
    const tone = path.join(folder, 'E2E tone.wav');
    writeWav(tone, 440, 22050);
    const before8 = await page.evaluate(() => ({ tracks: ST.state.project.tracks.length, instruments: ST.state.project.instruments.length }));
    const loaded = await page.evaluate(file => ST.loadSampleAsNew(file).then(result => result && { path: result.path, frames: result.frames, peaks: result.peaks?.length }), tone);
    assert.deepEqual(loaded, { path: tone, frames: 22050, peaks: 512 }, 'sample.load answered with the sample: ' + await toasts(page));
    const added = await page.evaluate(() => ({ tracks: ST.state.project.tracks.length, instruments: ST.state.project.instruments.length,
      samplePath: ST.state.project.instruments.at(-1).samplePath, overview: ST.state.samples?.[ST.state.project.instruments.length - 1]?.peaks?.length }));
    assert.deepEqual(added, { tracks: before8.tracks + 1, instruments: before8.instruments + 1, samplePath: tone, overview: 512 });
    // Notes for the new instrument on its new track, every beat of the first pattern.
    await page.evaluate(() => ST.mutate('E2E sample notes', () => {
      const project = ST.state.project, track = project.tracks.length - 1, instrument = project.instruments.length - 1;
      for (const pattern of project.patterns) {
        while (pattern.steps.length <= track) pattern.steps.push([]);
        for (let row = 0; row < pattern.length; row += 4) pattern.steps[track][row] = { note: 60, instrument, velocity: 120, gate: 2 };
      }
    }));
    await page.evaluate(() => ST.perform('play'));
    await page.waitForFunction(track => ST.state.status.playing === true && (ST.state.status.trackPeaks?.[track] || 0) > 0.01, added.tracks - 1, { timeout: 15000 });
    const samplePeak = await page.evaluate(track => ST.state.status.trackPeaks[track], added.tracks - 1);
    await page.evaluate(() => ST.perform('stop'));
    await page.waitForFunction(() => !ST.state.playing && ST.state.status.playing === false, null, { timeout: 5000 });
    pass(`a WAV file loaded into a new instrument from the page is decoded by the host and heard in playback (track meter ${samplePeak.toFixed(3)})`);

    const exportPath = path.join(folder, 'E2E export.wav');
    const songSeconds = await page.evaluate(() => { const project = ST.state.project;
      return project.sequence.reduce((rows, id) => rows + (project.patterns.find((pattern, index) => (pattern.id ?? index) === id)?.length || 0), 0) * 60 / project.bpm / project.linesPerBeat; });
    const exported = await page.evaluate(file => (ST.longRequest || ST.request).call(ST, 'audio.export', { mode: 'song', path: file, tail: 1, encoding: 'pcm16', sampleRate: 44100 })
      .then(result => ({ ok: result }), error => ({ error: error.message })), exportPath);
    assert.equal(exported.error, undefined, 'audio.export failed: ' + exported.error);
    assert.deepEqual([exported.ok.path, exported.ok.paths, exported.ok.container, exported.ok.encoding, exported.ok.sampleRate], [exportPath, [exportPath], 'wav', 'pcm16', 44100]);
    const wav = fs.readFileSync(exportPath);
    assert.equal(wav.toString('latin1', 0, 4) + wav.toString('latin1', 8, 12), 'RIFFWAVE');
    const dataAt = wav.indexOf('data', 12, 'latin1');
    assert.deepEqual([wav.readUInt16LE(22), wav.readUInt32LE(24), wav.readUInt16LE(34)], [2, 44100, 16], 'stereo 16-bit at 44.1 kHz');
    const exportedFrames = wav.readUInt32LE(dataAt + 4) / 4;
    assert.equal(exportedFrames, exported.ok.frames);
    assert.ok(Math.abs(exportedFrames / 44100 - (songSeconds + 1)) < 0.05, `the file is ${(exportedFrames / 44100).toFixed(2)} s long; the song is ${songSeconds.toFixed(2)} s plus a second of tail`);
    let exportPeak = 0;
    for (let at = dataAt + 8; at + 1 < wav.length; at += 2) exportPeak = Math.max(exportPeak, Math.abs(wav.readInt16LE(at)));
    assert.ok(exportPeak > 1000, `the exported song is not silent (peak ${exportPeak} of 32768)`);
    assert.deepEqual(fs.readdirSync(folder).filter(name => name.includes('E2E export')), ['E2E export.wav'], 'no staging folder is left next to the file');
    pass(`audio.export renders the song offline to ${path.basename(exportPath)}: ${(exportedFrames / 44100).toFixed(2)} s of 16-bit 44.1 kHz stereo, peak ${(exportPeak / 32768).toFixed(2)}`);
    // The sections below work on the demo song as it was: the sample's track is undone again.
    for (let step = 0; step < 6 && await page.evaluate(() => ST.state.project.tracks.length) > before8.tracks; step++) {
      await page.evaluate(() => ST.perform('undo'));
      await page.waitForTimeout(150);
    }
    assert.deepEqual(await page.evaluate(() => ({ tracks: ST.state.project.tracks.length, instruments: ST.state.project.instruments.length })), before8, 'Undo removes the sample instrument and its track');

    // ---- Plug-ins: scanned by container processes, inserted from the mixer, heard, saved and restored ----
    const plugin = installTestPlugin(bin, folder);
    const scanned = await request(page, 'plugins.scan', { folders: [plugin.plugins], wait: true });
    assert.equal(scanned.error, undefined, 'plugins.scan failed: ' + scanned.error);
    const listed = (await request(page, 'plugins.list')).ok.find(item => item.id === GAIN);
    assert.deepEqual([listed.name, listed.format, listed.type, listed.bundlePath], ['ST Test Gain', 'CLAP', 'effect', plugin.file]);
    assert.ok((await request(page, 'plugins.list')).ok.some(item => item.format === 'Builtin'), 'built-in effects are listed with the plug-ins');
    if (process.platform === 'linux') assert.doesNotMatch(fs.readFileSync(`/proc/${host.pid}/maps`, 'utf8'), /ST Test\.clap|stpc_clap_test_plugin/, 'listing must not load a plug-in into the host');
    pass('a folder of plug-ins is scanned by container processes and listed with the built-in effects, without loading a plug-in into the host');

    // The track that sounds most often in the demo song gets the plug-in, from the mixer's rack.
    await page.evaluate(() => ST.perform('play'));
    await page.waitForFunction(() => ST.state.status.playing === true && ST.state.status.peakL > 0.001, null, { timeout: 15000 });
    const activity = new Array(8).fill(0);
    for (const end = Date.now() + 2500; Date.now() < end; await page.waitForTimeout(40)) (await page.evaluate(() => ST.state.status.trackPeaks)).forEach((peak, index) => { if (peak > 0.001) activity[index]++; });
    const track = activity.indexOf(Math.max(...activity));
    assert.ok(activity[track] > 5, 'a track of the demo song is audible: ' + JSON.stringify(activity));
    await page.evaluate(index => { ST.state.track = index; ST.state.mixerOwner = null; }, track);
    await page.click('#view-mixer');
    await page.click('[data-studio-action="add-plugin"]');
    await page.waitForSelector(`[data-add-plugin="${GAIN}"]`, { timeout: 10000 });
    await page.click(`[data-add-plugin="${GAIN}"]`);
    await page.waitForFunction(id => (ST.state.project.audioPlugins || []).some(item => item.component === id && item.instanceID && item.state), GAIN, { timeout: 20000 })
      .catch(async error => { throw new Error('The plug-in was not inserted: ' + await toasts(page) + '\n' + error.message); });
    const inserted = await page.evaluate(id => ST.state.project.audioPlugins.find(item => item.component === id), GAIN);
    const target = { owner: inserted.owner, kind: 'effect', slot: inserted.slot };
    if (!target.owner) delete target.owner;
    assert.equal(inserted.track ?? track, track, 'the insert is on the selected track');
    assert.match(await toasts(page), /ST Test Gain added/);
    const routing = (await request(page, 'routing.status')).ok.inserts.find(item => item.instanceID === inserted.instanceID);
    assert.deepEqual([routing.available, routing.format], [true, 'CLAP']);
    pass(`the mixer's Add plug-in loads ${inserted.name} into a container and puts its descriptor (with its state) into the project`);

    // Heard: at gain zero the track's meter stays at zero while the song plays on; at half gain it moves again.
    const trackPeak = index => ST.state.status.trackPeaks[index];
    assert.ok(await loudest(page, trackPeak, track) > 0.001, 'the track sounds through the plug-in');
    const zero = await request(page, 'plugin.parameter', { ...target, address: '1', value: 0, normalized: true });
    assert.equal(zero.ok.value, 0, JSON.stringify(zero));
    await page.waitForFunction(index => ST.state.status.trackPeaks[index] === 0, track, { timeout: 5000 });
    assert.equal(await loudest(page, trackPeak, track), 0, 'gain zero silences the track');
    assert.ok(await loudest(page, () => ST.state.status.peakL) > 0.001, 'the rest of the song plays on');
    const half = await request(page, 'plugin.parameter', { ...target, address: '1', value: 0.25, normalized: true });
    assert.deepEqual([half.ok.value, half.ok.display], [0.5, '0.50 x']);
    assert.ok(await loudest(page, trackPeak, track) > 0.001, 'the track is back at half gain');
    await page.evaluate(() => ST.perform('stop'));
    pass('a parameter of the plug-in changes what is heard: gain zero silences its track, half gain brings it back');

    // ---- Routing: a return bus and a send made in the mixer reach the host's mixer ---------------
    await page.waitForSelector('[data-studio-action="new-return"]', { timeout: 10000 })
      .catch(async error => { throw new Error('The mixer does not offer + Return (routing.capabilities): ' + await toasts(page) + '\n' + error.message); });
    await page.click('[data-studio-action="new-return"]');
    await page.fill('#routing-bus-name', 'Room');
    await page.click('#routing-bus-create');
    await page.waitForFunction(() => (ST.state.project.routing?.buses || []).some(bus => bus.name === 'Room' && bus.type === 'return'), null, { timeout: 10000 })
      .catch(async error => { throw new Error('The return bus was not created: ' + await toasts(page) + '\n' + error.message); });
    const room = await page.evaluate(() => ST.state.project.routing.buses.find(bus => bus.name === 'Room'));
    const source = await page.evaluate(index => 'track:' + ST.state.project.tracks[index].routingID, track);
    const sent = await page.evaluate(([from, to]) => STRouting.transaction('Add send', draft => {
      draft.routing.sends.push({ id: 'e2e-send', source: from, destination: to, tap: 'postFader', gain: 1, enabled: true });
    }).then(() => 'applied', error => String(error.message || error)), [source, 'bus:' + room.id]);
    assert.equal(sent, 'applied');
    await page.waitForFunction(() => (ST.state.project.routing.sends || []).some(send => send.id === 'e2e-send'), null, { timeout: 10000 })
      .catch(async error => { throw new Error('The send was not applied: ' + await toasts(page) + '\n' + error.message); });
    const mixer = (await request(page, 'routing.status')).ok;
    assert.ok(mixer.strips.includes('bus:' + room.id), 'the host mixes the new bus: ' + JSON.stringify(mixer.strips));
    const offered = (await request(page, 'routing.capabilities')).ok;
    assert.ok(offered.exportTargets.some(item => item.owner === 'bus:' + room.id && item.kind === 'return' && item.name === 'Room'), 'the bus can be exported as a stem');
    await page.evaluate(() => ST.perform('play'));
    await page.waitForFunction(owner => Math.max(...(ST.state.status.busPeaks?.[owner] || [0])) > 0.001, 'bus:' + room.id, { timeout: 15000 })
      .catch(async error => { throw new Error('Nothing arrives on the return bus: ' + JSON.stringify(await page.evaluate(() => ST.state.status.busPeaks)) + '\n' + error.message); });
    await page.evaluate(() => ST.perform('stop'));
    pass('+ Return in the mixer creates a bus, a send feeds it, and the host meters signal on it');

    // Saved with its state (captured by the host at save), and restored when the file is opened again.
    const pluginSong = path.join(folder, 'Plug-in Song.supertracker');
    // This host process has opened no chooser yet: its next two are Save As and Open.
    fs.writeFileSync(path.join(folder, 'answers.json'), JSON.stringify([pluginSong, pluginSong]));
    assert.equal(await page.evaluate(() => ST.projectAction('saveAs')), true, 'save as failed: ' + await toasts(page));
    const stored = JSON.parse(fs.readFileSync(pluginSong, 'utf8')).audioPlugins.find(item => item.instanceID === inserted.instanceID);
    assert.ok(stored.state && stored.state !== inserted.state, 'the saved state is the one captured at save (gain 0.5), not the one from loading');
    await request(page, 'plugin.parameter', { ...target, address: '1', value: 0.9, normalized: true });   // an edit that is not saved
    // The page reads the plug-in's state before it replaces the session, finds the edit and asks.
    const fresh = page.evaluate(() => ST.projectAction('new'));
    await page.waitForSelector('#unsaved-discard', { timeout: 15000 }).catch(async error => { throw new Error('No question about the unsaved plug-in edit: ' + await toasts(page) + '\n' + error.message); });
    await page.click('#unsaved-discard');
    assert.equal(await fresh, true, 'new failed: ' + await toasts(page));
    await page.waitForFunction(() => !(ST.state.project.audioPlugins || []).length);
    assert.equal(await page.evaluate(() => ST.projectAction('open')), true, 'open failed: ' + await toasts(page));
    await page.waitForFunction(id => (ST.state.project.audioPlugins || []).some(item => item.instanceID === id), inserted.instanceID, { timeout: 10000 });
    let reopened;
    for (const end = Date.now() + 10000; Date.now() < end; await page.waitForTimeout(100)) {
      reopened = await request(page, 'plugin.parameters', { ...target, addresses: ['1'] });
      if (reopened.ok?.parameters?.[0]?.value === 0.5) break;
    }
    assert.equal(reopened.ok?.parameters?.[0]?.value, 0.5, 'the reopened project restores the saved gain: ' + JSON.stringify(reopened));
    assert.equal((await request(page, 'plugin.state', target)).ok.state, stored.state, 'the restored plug-in reads back the saved state');
    assert.deepEqual(problems.filter(text => !/not implemented yet|Failed to load resource/.test(text)), [], 'the page reported errors with plug-ins');
    pass('the project saves the plug-in with its state, and opening it again restores the plug-in with that state');
    assert.equal(await page.evaluate(() => ST.projectAction('demo')), true, 'loading the demo again failed: ' + await toasts(page));
    await page.click('#view-pattern');

    // ---- Line inputs: the null input's test tone into a track, from the mixer's Live input panel ----
    // The interface is chosen as Settings does it (the test tone is an input only tests ask for).
    const toneInput = await request(page, 'audio.configure', { input: 'null:tone' });
    assert.equal(toneInput.ok?.inputUid, 'null:tone', JSON.stringify(toneInput));
    await page.evaluate(audio => { ST.state.audio = { ...ST.state.audio, ...audio }; }, toneInput.ok);
    const lineTrack = 5, liveInput = `[data-line-input-mixer="${lineTrack}"]`;
    await page.evaluate(index => { ST.state.track = index; ST.state.mixerOwner = null; }, lineTrack);
    await page.click('#view-mixer');
    await page.waitForSelector(`${liveInput} [data-line-input-field="enabled"]:not([disabled])`, { timeout: 10000 })
      .catch(async error => { throw new Error('The mixer does not offer Live input for the selected track: ' + await toasts(page) + '\n' + error.message); });
    assert.match(await page.textContent(liveInput), /Live input from Test tone input/);
    assert.equal(await page.evaluate(index => ST.state.status.trackPeaks[index], lineTrack), 0, 'the track is silent while the song is stopped');
    await page.check(`${liveInput} [data-line-input-field="enabled"]`);
    await page.waitForFunction(index => ST.state.project.tracks[index].audioInput?.enabled === true && (ST.state.audio.lineInputs || []).some(item => item.track === index && item.state === 'live'), lineTrack, { timeout: 15000 })
      .catch(async error => { throw new Error('The line input did not go live: ' + JSON.stringify(await page.evaluate(() => [ST.state.audio.lineInputs, ST.state.audio.lineInputError])) + ' ' + await toasts(page) + '\n' + error.message); });
    assert.deepEqual(await page.evaluate(index => ST.state.project.tracks[index].audioInput, lineTrack), { gain: 1, enabled: true, deviceUID: 'null:tone', channels: [0] });
    // Heard without Play, on the track's meter: a quarter of full scale through the track's fader.
    const fader = await page.evaluate(index => ST.state.project.tracks[index].volume, lineTrack);
    await page.waitForFunction(index => ST.state.status.playing !== true && ST.state.status.trackPeaks[index] > 0.05, lineTrack, { timeout: 15000 })
      .catch(async error => { throw new Error('The line input is not on its track meter: ' + JSON.stringify(await page.evaluate(() => ST.state.status.trackPeaks)) + '\n' + error.message); });
    const tonePeak = await loudest(page, index => ST.state.status.trackPeaks[index], lineTrack, 500);
    assert.ok(Math.abs(tonePeak - 0.25 * fader) < 0.02, `the tone at ${0.25 * fader} on the track meter: ${tonePeak}`);
    await page.waitForSelector(`.line-input-mixer-state.is-live`, { timeout: 5000 });
    assert.equal((await request(page, 'audio.devices')).ok.lineInputs.find(item => item.track === lineTrack).latencyFrames, 512);
    // One Undo step takes the route out of the project, and with it out of the mixer.
    await page.evaluate(() => ST.perform('undo'));
    await page.waitForFunction(index => !ST.state.project.tracks[index].audioInput && ST.state.status.trackPeaks[index] === 0 && !(ST.state.audio.lineInputs || []).some(item => item.track === index), lineTrack, { timeout: 15000 })
      .catch(async error => { throw new Error('Undo did not take the line input away: ' + JSON.stringify(await page.evaluate(() => ST.state.audio.lineInputs)) + '\n' + error.message); });
    assert.equal((await request(page, 'audio.devices')).ok.inputRunning, false, 'no route, no open input');
    pass("Live input in the mixer routes the null input's tone into the selected track: it is in the project, heard on the track's meter, and gone with Undo");
    await page.click('#view-pattern');

    // ---- AI control: clients reach the page through the host's control socket ----------------
    const ping = ctl(bin, folder, 'app.ping');
    assert.equal(ping.status, 0, ping.stderr);
    assert.equal(JSON.parse(ping.stdout).app, 'SuperTracker');
    const summary = ctl(bin, folder, 'summary');
    assert.equal(summary.status, 0, summary.stderr);
    assert.equal(JSON.parse(summary.stdout).name, 'Midnight transmission', '--ctl reads the song that is open in the page');
    const mcp = mcpSession(bin, folder);
    const initialized = await mcp.request('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'e2e', version: '1' } });
    assert.equal(initialized.result.serverInfo.name, 'supertracker');
    mcp.notify('notifications/initialized');
    const tools = (await mcp.request('tools/list')).result.tools.map(tool => tool.name);
    for (const tool of ['get_song', 'read_pattern', 'write_steps']) assert.ok(tools.includes(tool), tool);
    const song = await mcp.tool('get_song');
    assert.equal(song.isError, false, song.text);
    assert.equal(JSON.parse(song.text.split('\n')[0]).name, 'Midnight transmission');
    // While a client is connected the page says so.
    await page.waitForFunction(() => document.querySelector('#agent-status')?.hidden === false, null, { timeout: 5000 });
    // An edit tool: the step appears in the page, as one undoable step.
    const undoBefore = await page.evaluate(() => ST.state.undo.length);
    const written = await mcp.tool('write_steps', { steps: [{ track: 3, row: 7, note: 'E-4', velocity: 99 }] });
    assert.equal(written.isError, false, written.text);
    // (Tools address the pattern the page shows.)
    const shown = () => ST.state.project.patterns.find(pattern => pattern.id === ST.state.project.activePattern) || ST.state.project.patterns[0];
    const step = await page.evaluate(`(${shown})().steps[3][7]`);
    assert.deepEqual([step.note, step.velocity], [64, 99], 'the step written through MCP is in the page');
    assert.equal(await page.evaluate(() => ST.state.undo.length), undoBefore + 1);
    const read = await mcp.tool('read_pattern', { format: 'grid' });
    assert.match(read.text, /E-4/);
    const viaCtl = ctl(bin, folder, 'writeSteps', { steps: [{ track: 3, row: 9, note: 'G-4' }] });
    assert.equal(viaCtl.status, 0, viaCtl.stderr);
    assert.equal(await page.evaluate(`(${shown})().steps[3][9].note`), 67, '--ctl edits the page too');
    pass('supertracker --ctl and an MCP session over supertracker --mcp read the song and write steps that appear in the page');

    // Turned off in Settings: clients are disconnected at once and refused from then on.
    await page.evaluate(() => ST.perform('settings'));
    await page.waitForSelector('#agent-enabled');
    assert.equal(await page.isChecked('#agent-enabled'), true);
    await page.click('#agent-enabled');
    await page.waitForFunction(() => /AI control turned off/.test(document.querySelector('#toast-region').innerText), null, { timeout: 5000 });
    assert.deepEqual((({ enabled, listening, clients }) => [enabled, listening, clients])((await request(page, 'agent.status')).ok), [false, false, 0]);
    assert.equal(await page.evaluate(() => document.querySelector('#agent-status').hidden), true, 'the "AI connected" badge is gone');
    const cutOff = await mcp.tool('get_song');
    assert.equal(cutOff.isError, true, 'the MCP session lost its connection: ' + cutOff.text);
    assert.match(cutOff.text, /Open SuperTracker first|AI control|turned off|not running/i);
    const refusedCtl = ctl(bin, folder, 'summary');
    assert.equal(refusedCtl.status, 1, 'supertracker --ctl is refused while AI control is off: ' + refusedCtl.stdout);
    // On again: the same MCP session reconnects by itself.
    await page.click('#agent-enabled');
    await page.waitForFunction(() => /AI control allowed/.test(document.querySelector('#toast-region').innerText), null, { timeout: 5000 });
    const back = await mcp.tool('get_song');
    assert.equal(back.isError, false, back.text);
    assert.equal(await mcp.close(), 0, 'the MCP server exits cleanly when its input ends');
    assert.deepEqual(problems.filter(text => !/not implemented yet|Failed to load resource/.test(text)), [], 'the page reported errors during AI control');
    pass('AI control turned off in Settings disconnects the clients; turned on again, the MCP session reconnects');
    assert.equal(await quit(host), 0);
    host = null;

    // ---- Termination signals end the host through the same shutdown as Quit ---------------------
    for (const [signal, number] of [['SIGTERM', 15], ['SIGINT', 2], ['SIGHUP', 1]]) {
      ({ host } = await startHost(bin, folder));
      const socket = JSON.parse(ctl(bin, folder, 'app.ping').stdout).socket;
      assert.ok(fs.existsSync(socket), 'the control socket is open: ' + socket);
      fs.rmSync(path.join(folder, 'support/Recovery.supertracker'), { force: true });
      host.kill(signal);
      const code = await Promise.race([host.exited, new Promise(resolve => setTimeout(() => resolve('timeout'), 10000))]);
      assert.equal(code, 128 + number, `${signal} ends the host with status ${128 + number}\n` + host.log);
      assert.match(host.log, new RegExp(`terminating on signal ${number}\\.`));
      assert.equal(fs.existsSync(socket), false, 'the control socket is closed and removed');
      assert.ok(fs.existsSync(path.join(folder, 'support/Recovery.supertracker')), 'the recovery copy is written first');
      assert.equal(ctl(bin, folder, 'app.ping').status, 1, 'no client reaches a host that has ended');
      host = null;
    }
    pass('SIGTERM, SIGINT and SIGHUP end the host with its services shut down (control socket removed, recovery copy written)');
    console.log(`\n${passed} checks passed.`);
  } finally {
    await browser?.close();
    if (host && host.exitCode === null) host.kill('SIGKILL');
    fs.rmSync(folder, { recursive: true, force: true });
  }
}

main().catch(error => { console.error('FAIL', error); process.exit(1); });
