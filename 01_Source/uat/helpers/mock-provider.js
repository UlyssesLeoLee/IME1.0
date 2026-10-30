'use strict';
// Local HTTPS "AI provider" for acceptance tests. It records every request (the number of requests is the evidence for
// "zero requests when AI is off / not authorised") and answers according to a per-test handler.
// A throw-away self-signed certificate is generated with OpenSSL (shipped with Git for Windows) and trusted by the
// VS Code extension host through NODE_EXTRA_CA_CERTS - no product code path is relaxed for testing.
const https = require('node:https');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {execFileSync} = require('node:child_process');

function findOpenssl() {
  const candidates = [process.env.OPENSSL_PATH, 'openssl', 'C:\\Program Files\\Git\\mingw64\\bin\\openssl.exe', 'D:\\Git\\mingw64\\bin\\openssl.exe', 'D:\\Program Files\\Git\\mingw64\\bin\\openssl.exe'].filter(Boolean);
  for (const c of candidates) { try { execFileSync(c, ['version'], {stdio: 'pipe'}); return c; } catch { /* next */ } }
  throw new Error('OpenSSL not found: set OPENSSL_PATH (Git for Windows ships mingw64/bin/openssl.exe)');
}
let cert = null;
function ensureCert() {
  if (cert) return cert;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pime-uat-cert-'));
  const keyFile = path.join(dir, 'key.pem'), certFile = path.join(dir, 'cert.pem');
  execFileSync(findOpenssl(), ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', keyFile, '-out', certFile, '-days', '2', '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1'], {stdio: 'pipe'});
  cert = {dir, keyFile, certFile, key: fs.readFileSync(keyFile), cert: fs.readFileSync(certFile)};
  return cert;
}

// handler({task, family, body, headers, n}) -> {status?, delayMs?, json?, text?, hang?}
//   task: 'full' | 'insert' | 'realtime' | 'unknown' (derived from the instructions the product sent)
async function startMock(name, handler) {
  const c = ensureCert();
  const state = {name, requests: [], handler, delayed: []};
  const server = https.createServer({key: c.key, cert: c.cert}, (req, res) => {
    const chunks = [];
    req.on('data', d => chunks.push(d));
    req.on('end', async () => {
      let body = {};
      try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch { /* keep {} */ }
      const family = req.url.includes('/chat/completions') ? 'openai-chat' : req.url.includes('/messages') ? 'anthropic-messages' : 'openai-responses';
      const instructions = family === 'openai-chat' ? (body.messages?.[0]?.content || '') : family === 'anthropic-messages' ? (body.system || '') : (body.instructions || '');
      const inputText = family === 'openai-chat' ? (body.messages?.[1]?.content || '') : family === 'anthropic-messages' ? (body.messages?.[0]?.content || '') : (body.input || '');
      const task = /fullFile/.test(instructions) ? 'full' : /at most 3 candidates/.test(instructions) ? 'insert' : /short completion/.test(instructions) ? 'realtime' : 'unknown';
      let input = {}; try { input = JSON.parse(inputText); } catch { /* not JSON */ }
      const record = {at: Date.now(), url: req.url, method: req.method, headers: req.headers, family, task, body, input, rawBody: Buffer.concat(chunks).toString('utf8')};
      state.requests.push(record);
      const plan = (await state.handler({task, family, body, input, headers: req.headers, n: state.requests.length, record})) || {};
      if (plan.hang) return; // never answers: the client must time out or be cancelled
      if (plan.delayMs) await new Promise(r => setTimeout(r, plan.delayMs));
      const status = plan.status || 200;
      if (status !== 200) { res.writeHead(status, {'content-type': 'application/json', ...(plan.headers || {})}); res.end(JSON.stringify({error: 'mock-error'})); return; }
      const text = plan.text ?? JSON.stringify(plan.json ?? {});
      const usage = {in: 11, out: 7};
      const payload = family === 'openai-chat' ? {choices: [{message: {content: text}}], usage: {prompt_tokens: usage.in, completion_tokens: usage.out}}
        : family === 'anthropic-messages' ? {content: [{type: 'text', text}], usage: {input_tokens: usage.in, output_tokens: usage.out}}
          : {output_text: text, usage: {input_tokens: usage.in, output_tokens: usage.out}};
      res.writeHead(200, {'content-type': 'application/json'}); res.end(JSON.stringify(payload));
    });
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const port = server.address().port;
  return {
    name, port, requests: state.requests, certFile: c.certFile,
    endpoint: family => `https://127.0.0.1:${port}${family === 'openai-chat' ? '/v1/chat/completions' : family === 'anthropic-messages' ? '/v1/messages' : '/v1/responses'}`,
    count: task => task ? state.requests.filter(r => r.task === task).length : state.requests.length,
    setHandler: h => { state.handler = h; },
    reset: () => { state.requests.length = 0; },
    close: () => new Promise(r => { server.closeAllConnections?.(); server.close(r); })
  };
}
module.exports = {startMock, ensureCert};
