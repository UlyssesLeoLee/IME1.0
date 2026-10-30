'use strict';
// Experiment only. Opens a webview with the CSP planned for the B-level design and loads media/panel.wasm into it.
// PROBE_CSP=with  -> script-src has 'wasm-unsafe-eval' (planned);  PROBE_CSP=without -> negative control.
const vscode = require('vscode');
const crypto = require('node:crypto');

exports.activate = ctx => {
  const open = () => {
    const withWasm = process.env.PROBE_CSP !== 'without';
    const panel = vscode.window.createWebviewPanel('pimeProbe', 'PIME WASM probe', vscode.ViewColumn.Active,
      {enableScripts: true, localResourceRoots: [vscode.Uri.joinPath(ctx.extensionUri, 'media')]});
    const nonce = crypto.randomBytes(16).toString('hex');
    const wasm = panel.webview.asWebviewUri(vscode.Uri.joinPath(ctx.extensionUri, 'media', 'panel.wasm'));
    const csp = `default-src 'none'; script-src 'nonce-${nonce}'${withWasm ? " 'wasm-unsafe-eval'" : ''}; style-src 'nonce-${nonce}'; connect-src ${panel.webview.cspSource};`;
    const model = JSON.stringify({tabs: {full: {hunks: [{hunk: 1}, {hunk: 2}, {hunk: 3}]}}});
    panel.webview.html = `<!DOCTYPE html><html><head><meta charset="UTF-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><title>probe</title>
<style nonce="${nonce}">body{font-family:sans-serif}</style></head><body><div id="out" data-state="pending">pending</div>
<script nonce="${nonce}">
(async () => {
  const vscodeApi = acquireVsCodeApi(), out = document.getElementById('out');
  const t0 = performance.now();
  try {
    const bytes = await (await fetch(${JSON.stringify(wasm.toString())})).arrayBuffer();
    const {instance} = await WebAssembly.instantiate(bytes, {});
    const t1 = performance.now();
    const e = instance.exports, input = new TextEncoder().encode(${JSON.stringify(model)});
    new Uint8Array(e.memory.buffer, e.input_ptr(), input.length).set(input);
    const n = e.render(input.length);
    const text = new TextDecoder().decode(new Uint8Array(e.memory.buffer, e.output_ptr(), n));
    out.textContent = text; out.dataset.state = 'ok'; out.dataset.ms = (t1 - t0).toFixed(1);
    vscodeApi.postMessage({type: 'wasm-ok', text, instantiateMs: +(t1 - t0).toFixed(1), wasmBytes: bytes.byteLength});
  } catch (err) {
    out.textContent = 'ERROR: ' + err.message; out.dataset.state = 'error';
    vscodeApi.postMessage({type: 'wasm-error', message: String(err.message)});
  }
})();
</script></body></html>`;
    panel.webview.onDidReceiveMessage(m => vscode.window.showInformationMessage('PROBE ' + JSON.stringify(m)));
  };
  ctx.subscriptions.push(vscode.commands.registerCommand('pimeProbe.open', open));
  setTimeout(open, 1500);
};
