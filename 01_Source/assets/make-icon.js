'use strict';
// Writes assets/pime.ico: the icon that the installer payload carries (the "Apps" entry of Windows and the files
// that are installed).
//
// The mark of the product is one file: crates/pime-setup/assets/ime1.0.ico (the logo of the design). It is copied
// here so that the payload of the installer and the window of the wizard carry the same picture. This script checks
// that the file really is a Windows icon: a broken .ico would show as an empty icon in Windows.
//
//   node assets/make-icon.js
const fs = require('node:fs');
const path = require('node:path');

const BRAND = path.join(__dirname, '..', 'crates', 'pime-setup', 'assets', 'ime1.0.ico');
const OUT = path.join(__dirname, 'pime.ico');

const bytes = fs.readFileSync(BRAND);
if (bytes.readUInt16LE(0) !== 0 || bytes.readUInt16LE(2) !== 1 || bytes.readUInt16LE(4) < 1) {
  console.error(`make-icon FAILED: ${BRAND} is not a Windows .ico`);
  process.exit(1);
}
fs.writeFileSync(OUT, bytes);
console.log(
  `make-icon: ${OUT} (${bytes.length} bytes, ${bytes.readUInt16LE(4)} pictures, the logo of the design)`
);
