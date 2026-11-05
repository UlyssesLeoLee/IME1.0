'use strict';
// Checks the picture of the native windows of the input method: it must exist, it must have the size of the screen and
// it must not be a flat rectangle (a window that painted nothing would look exactly like that, and no unit test can
// see it).
//
//   node tests/smoke/ime-ui-check.js [file.png]
//
// The picture is taken by tests/smoke/ime-ui-capture.ps1 while crates/pime-tsf/examples/ime_ui_probe.rs holds the
// settings window and the laser interface (the glow, the beam and the panel with the list) open.
//
// The picture alone cannot say whether the windows are the ones the product wants: the strip of the pages of the
// settings window is a few pixels tall, and a window without pages still makes a fine picture. The probe therefore
// writes what it found next to the picture (`probe-report.json`), and this script reads it: the strip must have the
// six pages of the product, the page of the plugins must be the one that is shown, and the list of the candidates must
// be in the panel of the light.
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

const file = process.argv[2] || path.join(__dirname, '..', '..', '..', '02_Verification', 'ime-ui', 'ime-windows.png');
const reportFile = path.join(path.dirname(file), 'probe-report.json');

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

// The hue of a colour, in degrees (the way `Rgb::hue` of the core computes it: 0 is red, 120 green, 240 blue).
// A colour without a hue (a grey) has none.
function hueOf(colour) {
  const hex = String(colour || '').replace('#', '');
  if (hex.length !== 6) return null;
  const r = parseInt(hex.slice(0, 2), 16) / 255;
  const g = parseInt(hex.slice(2, 4), 16) / 255;
  const b = parseInt(hex.slice(4, 6), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  if (delta <= 0) return null;
  let hue;
  if (max === r) hue = 60 * (((g - b) / delta) % 6);
  else if (max === g) hue = 60 * ((b - r) / delta + 2);
  else hue = 60 * ((r - g) / delta + 4);
  return ((hue % 360) + 360) % 360;
}

// The angle between two hues, the short way round the circle.
function angleDistance(a, b) {
  const difference = Math.abs(a - b) % 360;
  return difference > 180 ? 360 - difference : difference;
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

// ---- the spherical rainbow must not be in the corner any more -----------------------------------------------
// The user asked for the ball-shaped rainbow in the corner to be gone: the light on the edge of the screen is the
// only sign of the input method (the mouth is the handle). A picture of the whole screen is weak evidence on its
// own, so the picture is searched for the shape of the disc itself: a ring of strongly coloured pixels around a
// pale (white) core. The picture that still had the disc is the reference: 168 coloured pixels and 121 pale ones
// in the middle; the picture without it has 35 and 0.
{
  const x0 = header.width - 120;
  const x1 = header.width;
  const y0 = header.height - 140;
  const y1 = header.height - 40;
  const saturated = [];
  const pale = [];
  for (let y = y0; y < y1; y++) {
    for (let x = x0; x < x1; x++) {
      const i = (y * header.width + x) * channels;
      const r = pixels[i];
      const g = pixels[i + 1];
      const b = pixels[i + 2];
      const max = Math.max(r, g, b);
      const min = Math.min(r, g, b);
      const saturation = max === 0 ? 0 : (max - min) / max;
      const value = max / 255;
      if (saturation > 0.35 && value > 0.5) saturated.push(x, y);
      else if (saturation < 0.3 && value > 0.72) pale.push(x, y);
    }
  }
  let ring = 0;
  let core = 0;
  for (let k = 0; k < saturated.length; k += 2) {
    const cx = saturated[k];
    const cy = saturated[k + 1];
    let around = 0;
    let inside = 0;
    for (let m = 0; m < saturated.length; m += 2) {
      const dx = saturated[m] - cx;
      const dy = saturated[m + 1] - cy;
      if (dx * dx + dy * dy <= 256) around++;
    }
    for (let m = 0; m < pale.length; m += 2) {
      const dx = pale[m] - cx;
      const dy = pale[m + 1] - cy;
      if (dx * dx + dy * dy <= 64) inside++;
    }
    if (around > ring || (around === ring && inside > core)) {
      ring = around;
      core = inside;
    }
  }
  if (ring >= 30 && core >= 3) {
    fail(
      `the spherical rainbow is back in the corner (${ring} coloured pixels around a pale core of ${core} pixels)` +
        ': the logo window must not come back'
    );
  }
  console.log(`ok   no spherical rainbow in the corner (${ring} coloured pixels, ${core} pale ones in the middle)`);
}

// ---- what the probe found while the picture was taken ---------------------------------------------------------------------------------
// The strip of the pages is what makes the settings window usable, and the list of the candidates belongs to the
// panel of the light. Neither of them can be seen in a picture of a whole screen, so the probe says what it found.
if (!fs.existsSync(reportFile)) {
  fail(`the report of the probe is missing: ${reportFile} (the picture was taken without the probe?)`);
} else {
  let report = null;
  try {
    report = JSON.parse(fs.readFileSync(reportFile, 'utf8'));
  } catch (e) {
    fail(`the report of the probe is not readable: ${e.message}`);
  }
  if (report) {
    if (report.settingsWindow !== true) fail('the settings window was not opened');
    if (report.settingsPages !== 6) {
      fail(`the strip of the pages has ${report.settingsPages} pages, not the six of the product`);
    }
    if (report.selectedPageTitle !== '插件') {
      fail(`the page that is shown is "${report.selectedPageTitle}", not the page of the plugins`);
    }
    if (!(report.candidateRows > 0)) {
      fail('the list of the candidates is not in the panel of the light');
    }
    // The settings window has to be where the user can see it: in the middle of the screen. A window that opens in
    // the corner (or behind the other windows) is a window the user does not see at all.
    const [left, top, right, bottom] = report.settingsRect || [];
    const [screenW, screenH] = report.screen || [];
    if (!(left > 0 && top > 0 && right > left && bottom > top)) {
      fail(`the settings window is not on the screen: ${JSON.stringify(report.settingsRect)}`);
    } else {
      const cx = (left + right) / 2;
      const cy = (top + bottom) / 2;
      const inMiddle = (value, size) => Math.abs(value - size / 2) < size * 0.3;
      if (!inMiddle(cx, screenW) || !inMiddle(cy, screenH)) {
        fail(
          `the settings window is not in the middle of the screen (${left},${top})-(${right},${bottom}) ` +
          `on a ${screenW}x${screenH} screen: it opens in the corner, where the user does not see it`
        );
      } else {
        console.log(`ok   the settings window is in the middle of the screen (${left},${top})`);
      }
    }
    console.log(
      `ok   the probe found ${report.settingsPages} pages of the settings (${report.selectedPageTitle} shown) ` +
      `and ${report.candidateRows} rows of the list in the panel of the light`
    );

    // ---- the mouths of light -------------------------------------------------------------------------------
    // The user asked for one mouth of light per source of input: the human at the keyboard and every agent, each in
    // a colour that tells the agents apart ("给它们开颜色区分比较明显的随机rgb激光口"). The probe writes down what
    // it found, and this is where it is checked: the colours have to be different and far apart, and the mouths have
    // to be spread over the edges of the screen.
    const mouths = Array.isArray(report.mouths) ? report.mouths : [];
    if (mouths.length < 2) {
      fail(`there are only ${mouths.length} mouths of light on the screen: the human and every agent need one`);
    }
    const mouthColours = new Set(mouths.map((mouth) => String(mouth.colour || '').toUpperCase()));
    if (mouthColours.size !== mouths.length) {
      fail(
        `two mouths of light have the same colour: ${JSON.stringify(mouths.map((mouth) => mouth.colour))}`
      );
    }
    const edges = new Set(mouths.map((mouth) => String(mouth.edge || '')));
    if (edges.size < 2) {
      fail(
        `the mouths of light are all on the same edge (${[...edges].join(', ')}): ` +
        'they have to be spread over the screen'
      );
    }
    const agents = mouths.filter((mouth) => String(mouth.key || '').startsWith('agent:'));
    if (agents.length < 2) {
      fail(`there are only ${agents.length} mouths of agents on the screen: the check needs two of them`);
    }
    // The threshold: 45 degrees is a quarter of the colour wheel. The eye tells two hues apart at about 30 degrees, and
    // the colours of the agents also differ in brightness, so 45 degrees is the distance at which two mouths are
    // told apart at a glance - even in a slit of light that is one pixel wide.
    const MIN_HUE_DISTANCE = 45;
    for (let i = 0; i < agents.length; i++) {
      for (let j = i + 1; j < agents.length; j++) {
        const first = hueOf(agents[i].colour);
        const second = hueOf(agents[j].colour);
        if (first === null || second === null) {
          fail(
            `the colour of a mouth of an agent has no hue: ${agents[i].colour} and ${agents[j].colour}`
          );
        }
        const distance = angleDistance(first, second);
        if (distance < MIN_HUE_DISTANCE) {
          fail(
            `the mouths of the agents "${agents[i].key}" and "${agents[j].key}" are only ` +
            `${distance.toFixed(1)} degrees apart (${MIN_HUE_DISTANCE} are needed): ` +
            'the user cannot tell them apart'
          );
        }
      }
    }
    console.log(
      `ok   ${mouths.length} mouths of light (${mouthColours.size} colours, on the edges ${[...edges].join(', ')}), ` +
      `the colours of the agents are at least ${MIN_HUE_DISTANCE} degrees apart`
    );
  }
}

console.log(`OK: ${file} - ${header.width}x${header.height}, ${colours.size} colours, ${bytes.length} bytes`);
