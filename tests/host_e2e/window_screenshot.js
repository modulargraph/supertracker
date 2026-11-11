#!/usr/bin/env node
// Screenshot checks of the window smoke test (window_smoke.sh calls this; see README.md).
//
//   node window_screenshot.js <supertracker> window <width> <height> <folder>
//       The window is open (the control socket is found through the environment, as the host found
//       it). Asks for screenshots with `--ctl app.screenshot` and with the MCP tool `screenshot`
//       over `--mcp`, decodes the PNG files and checks their sizes against the window's size on the
//       display (<width> x <height>, which includes the menu bar) and that they show content.
//       Writes the pictures into <folder> (window.png, window-640.png, window-mcp-800.png).
//   node window_screenshot.js <supertracker> changed <before.png> <after.png>
//       Takes a screenshot, writes it to <after.png> and checks that it differs visibly from
//       <before.png>: the picture follows what the window shows.
//   node window_screenshot.js <supertracker> no-window
//       The host runs without a window (--serve): the request is refused with a message.
//
// Prints PASS/FAIL lines like window_smoke.sh and exits with the number of failures.
'use strict';
const { spawn, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

const [bin, mode, ...rest] = process.argv.slice(2);
let failures = 0;
function check(name, test) {
  try {
    const detail = test();
    console.log(`PASS ${name}${detail ? ` (${detail})` : ''}`);
    return true;
  } catch (error) {
    console.log(`FAIL ${name}: ${error.message}`);
    failures += 1;
    return false;
  }
}
function expect(condition, message) {
  if (!condition) throw new Error(message);
}

/** Decodes a PNG file (8-bit RGB or RGBA, not interlaced) into {width, height, channels, pixels}; checks every chunk's CRC. */
function decodePng(file) {
  expect(file.length > 8 && file.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])), 'not a PNG file (wrong signature)');
  let header = null;
  let ended = false;
  const data = [];
  for (let at = 8; at < file.length;) {
    expect(at + 12 <= file.length, 'a chunk is cut off');
    const length = file.readUInt32BE(at);
    const type = file.toString('latin1', at + 4, at + 8);
    expect(at + 12 + length <= file.length, `the ${type} chunk is cut off`);
    const body = file.subarray(at + 8, at + 8 + length);
    expect(zlib.crc32(file.subarray(at + 4, at + 8 + length)) === file.readUInt32BE(at + 8 + length), `the ${type} chunk has a wrong CRC`);
    if (type === 'IHDR') {
      header = { width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8], colour: body[9], interlace: body[12] };
    } else if (type === 'IDAT') {
      data.push(body);
    } else if (type === 'IEND') {
      ended = true;
    }
    at += 12 + length;
  }
  expect(header && ended, 'the PNG file has no header or no end');
  expect(header.depth === 8 && (header.colour === 2 || header.colour === 6) && header.interlace === 0, `unexpected PNG format ${JSON.stringify(header)}`);
  const channels = header.colour === 2 ? 3 : 4;
  const stride = header.width * channels;
  const raw = zlib.inflateSync(Buffer.concat(data));
  expect(raw.length === (stride + 1) * header.height, `the PNG file holds ${raw.length} bytes of pixels, not ${(stride + 1) * header.height}`);
  const pixels = Buffer.alloc(stride * header.height);
  for (let y = 0; y < header.height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const out = pixels.subarray(y * stride, (y + 1) * stride);
    const above = y > 0 ? pixels.subarray((y - 1) * stride, y * stride) : null;
    for (let x = 0; x < stride; x += 1) {
      const left = x >= channels ? out[x - channels] : 0;
      const up = above ? above[x] : 0;
      const upLeft = above && x >= channels ? above[x - channels] : 0;
      let predicted = 0;
      if (filter === 1) predicted = left;
      else if (filter === 2) predicted = up;
      else if (filter === 3) predicted = (left + up) >> 1;
      else if (filter === 4) {
        const estimate = left + up - upLeft;
        const [toLeft, toUp, toUpLeft] = [Math.abs(estimate - left), Math.abs(estimate - up), Math.abs(estimate - upLeft)];
        predicted = toLeft <= toUp && toLeft <= toUpLeft ? left : toUp <= toUpLeft ? up : upLeft;
      } else expect(filter === 0, `unknown PNG filter ${filter}`);
      out[x] = (line[x] + predicted) & 255;
    }
  }
  return { width: header.width, height: header.height, channels, pixels };
}

/** How many different colours a picture has, and the share of its most frequent one. */
function colours(picture) {
  const counts = new Map();
  for (let at = 0; at < picture.pixels.length; at += picture.channels) {
    const colour = (picture.pixels[at] << 16) | (picture.pixels[at + 1] << 8) | picture.pixels[at + 2];
    counts.set(colour, (counts.get(colour) || 0) + 1);
  }
  const most = Math.max(...counts.values());
  return { distinct: counts.size, share: most / (picture.width * picture.height) };
}

/** `supertracker --ctl app.screenshot [params]`: the decoded reply, with the PNG file in `file`. */
function screenshotByCtl(params) {
  const run = spawnSync(bin, ['--ctl', 'app.screenshot', ...(params ? [JSON.stringify(params)] : [])], { encoding: 'utf8', timeout: 60000, maxBuffer: 256 * 1024 * 1024 });
  expect(run.status === 0, `--ctl app.screenshot exited with ${run.status}: ${(run.stderr || '').trim() || run.error}`);
  const reply = JSON.parse(run.stdout);
  expect(reply.mimeType === 'image/png' && typeof reply.data === 'string', `unexpected reply ${run.stdout.slice(0, 200)}`);
  expect(/^[A-Za-z0-9+/]+={0,2}$/.test(reply.data) && reply.data.length % 4 === 0, 'the data is not standard base64');
  return { ...reply, file: Buffer.from(reply.data, 'base64') };
}

/** One MCP session over `supertracker --mcp`: initialize, then the tool `screenshot` with `args`. Resolves to the tool's result. */
function screenshotByMcp(args) {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, ['--mcp'], { stdio: ['pipe', 'pipe', 'inherit'] });
    const timer = setTimeout(() => { child.kill(); reject(new Error('the MCP server did not answer in 60 s')); }, 60000);
    let buffered = '';
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', text => {
      buffered += text;
      for (let end = buffered.indexOf('\n'); end >= 0; end = buffered.indexOf('\n')) {
        const message = JSON.parse(buffered.slice(0, end));
        buffered = buffered.slice(end + 1);
        if (message.id === 1) {
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'screenshot', arguments: args } }) + '\n');
        } else if (message.id === 2) {
          clearTimeout(timer);
          child.stdin.end();
          resolve(message.result || message);
        }
      }
    });
    child.on('error', reject);
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'window-smoke', version: '1' } } }) + '\n');
  });
}

async function windowChecks(windowWidth, windowHeight, folder) {
  let whole = null;
  check('--ctl app.screenshot returns a PNG file of the window\'s content', () => {
    const reply = screenshotByCtl();
    fs.writeFileSync(path.join(folder, 'window.png'), reply.file);
    const picture = decodePng(reply.file);
    expect(reply.width === picture.width && reply.height === picture.height, `the reply says ${reply.width}x${reply.height}, the file ${picture.width}x${picture.height}`);
    // The web view fills the window below the menu bar.
    expect(picture.width === windowWidth, `the picture is ${picture.width} px wide, the window ${windowWidth}`);
    expect(picture.height <= windowHeight && picture.height >= windowHeight - 80, `the picture is ${picture.height} px high, the window ${windowHeight} (with its menu bar)`);
    whole = picture;
    return `${picture.width}x${picture.height} in a ${windowWidth}x${windowHeight} window, ${reply.file.length} bytes`;
  });
  if (whole) {
    check('the picture shows the interface, not one flat colour', () => {
      const { distinct, share } = colours(whole);
      expect(distinct >= 64 && share < 0.9, `${distinct} different colours, the most frequent on ${(share * 100).toFixed(1)}% of the pixels`);
      return `${distinct} colours, the most frequent on ${(share * 100).toFixed(1)}% of the pixels`;
    });
    check('a requested width scales the picture down and keeps its proportions', () => {
      const reply = screenshotByCtl({ width: 640 });
      fs.writeFileSync(path.join(folder, 'window-640.png'), reply.file);
      const picture = decodePng(reply.file);
      const height = Math.round(whole.height * 640 / whole.width);
      expect(picture.width === 640 && picture.height === height, `${picture.width}x${picture.height}, expected 640x${height}`);
      expect(reply.width === 640 && reply.height === height, `the reply says ${reply.width}x${reply.height}`);
      const { distinct, share } = colours(picture);
      expect(distinct >= 64 && share < 0.9, `${distinct} different colours after scaling`);
      return `${picture.width}x${picture.height}`;
    });
    check('a width beyond the window\'s does not enlarge the picture', () => {
      const reply = screenshotByCtl({ width: 2560 });
      expect(reply.width === whole.width && reply.height === whole.height, `${reply.width}x${reply.height}`);
      return `${reply.width}x${reply.height}`;
    });
  }
  const result = await screenshotByMcp({ width: 800 }).catch(error => ({ failed: error.message }));
  check('the MCP tool screenshot returns the picture as image content', () => {
    expect(!result.failed && !result.isError, `the tool failed: ${result.failed || JSON.stringify(result.content)}`);
    const [image, text] = result.content;
    expect(image.type === 'image' && image.mimeType === 'image/png' && typeof image.data === 'string', `unexpected content ${JSON.stringify(result.content).slice(0, 200)}`);
    const file = Buffer.from(image.data, 'base64');
    fs.writeFileSync(path.join(folder, 'window-mcp-800.png'), file);
    const picture = decodePng(file);
    expect(picture.width === 800, `the picture is ${picture.width} px wide, not 800`);
    if (whole) expect(picture.height === Math.round(whole.height * 800 / whole.width), `the picture is ${picture.height} px high`);
    expect(text.type === 'text' && text.text === `SuperTracker window, ${picture.width}×${picture.height} px.`, `the text is ${JSON.stringify(text)}`);
    expect(colours(picture).distinct >= 64, 'the picture is flat');
    return text.text;
  });
}

function changedChecks(before, after) {
  check('a screenshot taken after the window changed shows the change', () => {
    const reply = screenshotByCtl();
    fs.writeFileSync(after, reply.file);
    const [old, now] = [decodePng(fs.readFileSync(before)), decodePng(reply.file)];
    expect(old.width === now.width && old.height === now.height, `the size changed from ${old.width}x${old.height} to ${now.width}x${now.height}`);
    let different = 0;
    for (let at = 0; at < now.pixels.length; at += now.channels) {
      if (now.pixels[at] !== old.pixels[at] || now.pixels[at + 1] !== old.pixels[at + 1] || now.pixels[at + 2] !== old.pixels[at + 2]) different += 1;
    }
    const share = different / (now.width * now.height);
    expect(share >= 0.01, `only ${(share * 100).toFixed(2)}% of the pixels differ`);
    return `${(share * 100).toFixed(1)}% of the pixels differ`;
  });
}

function noWindowChecks() {
  check('a host without a window refuses the screenshot with a message', () => {
    const run = spawnSync(bin, ['--ctl', 'app.screenshot'], { encoding: 'utf8', timeout: 60000 });
    const said = (run.stderr || '').trim();
    expect(run.status === 1 && /without a window .*no window to take a screenshot of/.test(said), `exit status ${run.status}, message: ${said || run.stdout.slice(0, 200)}`);
    return said;
  });
}

(async () => {
  if (mode === 'window' && rest.length === 3) await windowChecks(Number(rest[0]), Number(rest[1]), rest[2]);
  else if (mode === 'changed' && rest.length === 2) changedChecks(rest[0], rest[1]);
  else if (mode === 'no-window' && rest.length === 0) noWindowChecks();
  else {
    console.error('usage: window_screenshot.js <supertracker> window <width> <height> <folder> | changed <before.png> <after.png> | no-window');
    process.exit(64);
  }
  process.exit(failures);
})();
