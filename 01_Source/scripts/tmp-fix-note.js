// Temporary helper: the JavaScript evidence of the traceability report was re-run, so the note that said
// it was reused has to go; then the claims and the manifest are refreshed.
const fs = require('node:fs');
const root = 'E:\\IME1.0';
const readme = `${root}\\README_交付说明.md`;

let r = fs.readFileSync(readme, 'utf8');
const old = `   * **追溯报告（\`02_Verification/test-execution-report.*\`）的证据来源**：**TSF 与 UAT 是本轮新跑的**；
     **JavaScript 那一半取自 \`node-test-result.txt\`（2026-11-11 那一轮）**——因为**本轮 JS 测试文件一行没改**，
     而机器被另一会话占满时重跑一次 JS 套件要 40 分钟以上（两次都撞了超时）。

`;
const fresh = `   * **追溯报告（\`02_Verification/test-execution-report.*\`）的四路证据都是本轮跑的**：JS 327/327、Rust 458/458、
     TSF 367/367、UAT 58/58 ⇒ **UT 76/77、IT 43/48、ST 35/43，0 失败**，覆盖率 **154/168 = 91.7%**。

`;
if (r.includes(old)) { r = r.replace(old, fresh); fs.writeFileSync(readme, r, 'utf8'); console.log('the note about the reused JS evidence is replaced'); }
else console.log('the note was not found (already updated?)');
