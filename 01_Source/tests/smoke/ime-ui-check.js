'use strict';
// Checks the picture of the native windows of the input method: it must exist, it must have the size of the screen and
// it must not be a flat rectangle (a window that painted nothing would look exactly like that, and no unit test can
// see it).
//
//   node tests/smoke/ime-ui-check.js [file.png]
//
// The picture is taken by tests/smoke/ime-ui-capture.ps1 while crates/pime-tsf/examples/ime_ui_probe.rs holds the
// candidate window, the floating bar and the settings window open.
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

const file = process.argv[2] || path.join(__dirname, '..', '..', '..', '02_Verification', 'ime-ui', 'ime-windows.png');

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

if (!fs.existsSync(file)) fail(`the picture is missing: ${file}`);
const bytes = fs.readFileSync(file);
if (bytes.length < 4096) fail(`the picture is too small to be a screen (${bytes.length} bytes)`);

// the PNG chunks
const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
if (!bytes.subarray(0, 8).equals(signature)) fail('the file is not a PNG');
let offset = 8;
let header = null;
const idat = [];
while (offset + 8 <= bytes.length) {
  const length = bytes.readUInt32BE(offset);
  const kind = bytes.toString('ascii', offset + 4, offset + 8);
  const body = bytes.subarray(offset + 8, offset + 8 + length);
  if (kind === 'IHDR') header = {width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8], color: body[9]};
  if (kind === 'IDAT') idat.push(body);
  offset += 12 + length;
  if (kind === 'IEND') break;
}
if (!header) fail('the PNG has no header');
if (header.width < 640 || header.height < 480) fail(`the picture is too small: ${header.width}x${header.height}`);

// the pixels: the PNG filter of every row has to be undone before the colours can be counted
const raw = zlib.inflateSync(Buffer.concat(idat));
const channels = header.color === 2 ? 3 : 4;
const stride = header.width * channels;
const pixels = Buffer.alloc(stride * header.height);
let previous = Buffer.alloc(stride);
for (let y = 0; y < header.height; y++) {
  const filter = raw[y * (stride + 1)];
  const line = raw.subarray(y * (stride + 1) + 1, y * (stride + 1) + 1 + stride);
  const out = pixels.subarray(y * stride, (y + 1) * stride);
  for (let x = 0; x < stride; x++) {
    const a = x >= channels ? out[x - channels] : 0;
    const b = previous[x];
    const c = x >= channels ? previous[x - channels] : 0;
    let value = line[x];
    if (filter === 1) value += a;
    else if (filter === 2) value += b;
    else if (filter === 3) value += (a + b) >> 1;
    else if (filter === 4) {
      const p = a + b - c;
      const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
      value += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
    }
    out[x] = value & 0xff;
  }
  previous = out;
}

// the colours that really occur: a flat picture has one, a screen with three windows has thousands
const colours = new Set();
for (let i = 0; i < pixels.length; i += channels * 7) {
  colours.add(`${pixels[i]},${pixels[i + 1]},${pixels[i + 2]}`);
  if (colours.size > 20000) break;
}
if (colours.size < 50) fail(`the picture looks flat (${colours.size} colours): the windows probably painted nothing`);

console.log(`OK: ${file} - ${header.width}x${header.height}, ${colours.size} colours, ${bytes.length} bytes`);
