'use strict';
// UI models that are independent of VS Code: accessible candidate rows (UT-057), overlay placement (UT-055) and the
// environment check (UT-077). Meaning is always carried by text (source, validation, position) - never by colour.
const {t} = require('./i18n');

// One row of the candidate list: name, position, source, validation and the available action, all as text.
function candidateRow(locale, c, index, total) {
  const sources = c.sources && c.sources.length ? c.sources : [c.source];
  const source = t(locale, 'src.' + sources[0]) + (sources.length > 1 ? ` +${sources.length - 1}` : '');
  const validation = t(locale, 'val.' + c.validation);
  const detail = [c.explain, ...(c.assumptions || [])].filter(Boolean).join(' · ');
  return {
    label: c.label,
    description: `[${source}] ${validation}`,
    detail,
    ariaLabel: `${index} / ${total}: ${c.label}, ${source}, ${validation}${c.text == null ? ', not insertable' : ''}`,
    insertable: c.text != null
  };
}

// Places a candidate window inside the monitor work area that contains the caret. Negative coordinates (monitors
// left of / above the primary) are legal. Returns null when the caret is on no monitor: hide, never steal focus.
function placeOverlay({caret, size, monitors, dpi = 100}) {
  const scale = Math.min(250, Math.max(100, dpi)) / 100;
  const w = Math.round(size.w * scale), h = Math.round(size.h * scale);
  const mon = monitors.find(m => caret.x >= m.x && caret.x < m.x + m.w && caret.y >= m.y && caret.y < m.y + m.h);
  if (!mon || w > mon.w || h > mon.h) return null;
  let x = caret.x, y = caret.y + Math.round(18 * scale); // below the caret line
  if (y + h > mon.y + mon.h) y = caret.y - h - 2;          // not enough room below: flip above
  if (x + w > mon.x + mon.w) x = mon.x + mon.w - w;        // clamp right edge
  x = Math.max(mon.x, x); y = Math.max(mon.y, y);
  return {x, y, w, h};
}

// Supported: Windows 11 (build >= 22000) on x64, display scaling 100-250 %. Anything else is reported, not guessed.
function checkEnvironment({platform, arch, build, dpi}) {
  const reasons = [];
  if (platform !== 'win32') reasons.push('Windows is required');
  if (arch !== 'x64') reasons.push('x64 is required');
  if (!Number.isInteger(build) || build < 22000) reasons.push('Windows 11 (build 22000 or newer) is required');
  if (dpi !== undefined && (!Number.isFinite(dpi) || dpi < 100 || dpi > 250)) reasons.push('display scaling must be between 100% and 250%'); // dpi is unknown inside VS Code: skipped, not guessed
  return {supported: reasons.length === 0, reasons};
}
module.exports = {candidateRow, placeOverlay, checkEnvironment};
