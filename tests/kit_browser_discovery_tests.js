'use strict';
// Real kit browser behavior with synthetic catalogue replies; no native/device work.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const template = fs.readFileSync(path.join(__dirname, 'kit_browser_tests.js'), 'utf8');
const prefix = template.slice(0, template.indexOf('(async () => {\n  const h = await fresh();'));
const { fresh } = new Function('require', '__dirname', prefix + '\nreturn {fresh};')(require, __dirname);

(async () => {
  const h = await fresh();
  assert.match(h.markup, /Where are my groups\?/);
  assert.match(h.markup, /Library lists the Maschine groups in your indexed folders by expansion/);
  assert.match(h.markup, /Saved SuperTracker kits are under <strong>My kits<\/strong>/);
  assert.match(h.markup, /Expansions · kits · details|kitb-products/);
  h.node('kitb-help').open = true;
  h.node('kitb-help').handlers.toggle({ target: h.node('kitb-help') });
  h.node('kitb-help-toggle').focus();
  h.browser.controller.toggleFavorites(); await h.run();
  assert.match(h.markup, /id="kitb-help" class="kitb-help" open/);
  assert.equal(h.focused, 'kitb-help-toggle', 'catalogue refresh preserves focus and open instructions');
  assert.equal(h.loads.length, 0, 'reading help never imports a group');
  console.log('PASS Maschine group guidance explains Library, My kits and folders without changing the project');

  const empty = await fresh();
  empty.state.library = { roots: [{ path: '/Expansion', available: true }] };
  empty.requestHandler = command => command === 'library.kits.browse' ? { items: [], total: 0, products: [], counts: { library: 0, user: 0 } } : undefined;
  await empty.browser.search();
  assert.match(empty.markup, /No groups indexed yet/);
  assert.match(empty.markup, /Rescan your existing library to find Maschine groups \(.mxgrp\)/);
  assert.match(empty.markup, /class="btn primary" id="kitb-rescan-empty"/);
  await empty.node('kitb-rescan-empty').handlers.click();
  assert(empty.calls.some(call => call.command === 'library.scan'));
  assert.equal(empty.calls.some(call => call.command === 'library.chooseRoot'), false, 'existing roots can be rescanned without another folder chooser');
  empty.state.library.roots[0].available = false; empty.render();
  assert.match(empty.markup, /Reconnect unavailable drives first/);
  console.log('PASS an older sample-only index offers direct rescan and disconnected-drive guidance');

  empty.requestHandler = command => command === 'library.kits.browse' ? { items: [], total: 0, kitIndexUpgradePending: true, kitIndexUpgradeScanning: true } : undefined;
  await empty.browser.search();
  assert.match(empty.markup, /Finding Maschine groups…/);
  assert.match(empty.markup, /id="kitb-rescan-empty" disabled>Scanning library…/);
  empty.browser.event('library.progress', { scanning: false, kitIndexUpgradePending: false, kitIndexUpgradeScanning: false });
  assert.match(empty.markup, /id="kitb-rescan-empty" >Rescan library/);
  empty.state.library.roots[0].available = true;
  empty.browser.event('library.results', { scanning: false, kitIndexUpgradePending: true, kitIndexUpgradeScanning: false, kitIndexUpgradeOfflineRoots: ['/Expansion'] });
  assert.doesNotMatch(empty.markup, /Finding Maschine groups…/);
  assert.match(empty.markup, /Reconnect unavailable drives first/);
  assert.match(empty.markup, /id="kitb-rescan-empty" >Rescan library/);
  empty.browser.event('library.progress', { scanning: false, kitIndexUpgradePending: true, kitIndexUpgradeScanning: false, kitIndexUpgradeOfflineRoots: [] });
  assert.doesNotMatch(empty.markup, /Reconnect unavailable drives first/);
  assert.doesNotMatch(empty.markup, /Finding Maschine groups…/, 'pending failed/cancelled scans do not look active');
  empty.browser.controller.toggleUser(); await empty.run();
  assert.match(empty.markup, /No saved kits yet/);
  assert.match(empty.markup, /id="kitb-new-empty"/);
  empty.browser.controller.choose('tags', 'Kits\u001fHip Hop'); await empty.run();
  assert.match(empty.markup, /No matching groups or kits/);
  assert.match(empty.markup, /id="kitb-clear-empty"/);
  console.log('PASS automatic kit-index discovery, empty My kits and restrictive filters have distinct recovery guidance');
  console.log('3 kit browser discovery groups passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
