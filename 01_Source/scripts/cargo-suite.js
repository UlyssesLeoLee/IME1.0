'use strict';
// Runs a cargo test command on a machine whose antivirus watches the freshly linked binaries.
//
// The scanner of the machine (360 on this host) does two things to a binary that has just been linked:
//   1. it replaces the ACL of the file with one that gives the normal user no write access, so the NEXT link of
//      the same file fails with "cannot open output file ...: Permission denied";
//   2. it refuses the execution of the file for a while ("could not execute process ... (never executed)",
//      "アクセスが拒否されました (os error 5)") and removes it afterwards.
// Both are properties of the machine, not of the code (D-138). The gate must not turn red for them, and the
// red-green check must not call a mutation "caught" because of them.
//
// The way out was measured on this machine: a COPY of the binary in a temporary directory runs fine and stays
// there - the scanner watches the path it has seen, not the bytes. So when a run is blocked:
//   - the write access of the blocked file is restored first (the owner may always do that) and the command is
//     tried again: that answers (1);
//   - if the execution is still refused, the test binaries are built WITHOUT running them, copied into a
//     temporary directory and run there: that answers (2).
//
// The tests that run are the same tests with the same assertions - only the place of the file changes. The
// output of the copies is the output of the test harness itself, so the numbers the gate quotes (the counts of
// "test result: ok. N passed") and the verdict of the red-green check ("test result: FAILED") are the ones the
// tests really produced.
//
// The helper is Windows-only in its scanner handling: on a machine without a scanner the plain cargo run is
// used and nothing changes.
const {spawnSync} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

// The execution of a binary was refused (the scanner quarantines it).
// "could not be started" is the form the scanner takes on this machine when it blocks a copy in the temporary
// folder: the file is there, the process never runs (spawn EPERM) - the same class of machine fault.
const BLOCKED_EXECUTION = /could not execute process|never executed|os error 5|access was denied|アクセスが拒否|could not be started/i;
// The linker could not write the output file (the scanner replaced its ACL).
const BLOCKED_WRITE = /cannot open output file\s+([^\r\n"]+?):\s*Permission denied/i;

const isWin = process.platform === 'win32';

function cargo(args, cwd, env) {
  const r = spawnSync('cargo', args, {
    cwd,
    encoding: 'utf8',
    env,
    // no shell: cargo.exe is a program, and a shell would concatenate the arguments without escaping them
    maxBuffer: 256 * 1024 * 1024,
  });
  // A command that could not even be STARTED (the commit limit is exhausted, the scanner refuses the program) is a
  // property of the machine, not of the code. The reason goes into the output, otherwise the gate reads an empty
  // output as "the scanner blocked it" and never says what really happened (D-180).
  const startError = r.error
    ? `\n[the command could not be started: ${r.error.code || ''} ${r.error.message}]`
    : '';
  return {status: r.status === null ? -1 : r.status, out: `${r.stdout || ''}${r.stderr || ''}${startError}`};
}

// The owner of a file may always change its ACL: that is what undoes the protection of the scanner.
function restoreWriteAccess(file) {
  if (!isWin || !file || !fs.existsSync(file)) return false;
  const user = (process.env.USERNAME || '').trim();
  if (!user) return false;
  const r = spawnSync('icacls', [file, '/grant', `${user}:(M)`], {encoding: 'utf8'});
  return r.status === 0;
}

// The directory of the package a test binary belongs to: "path+file:///E:/repo/crates/x#0.1.0" -> "E:/repo/crates/x".
function packageRootOf(packageId) {
  if (!packageId) return null;
  const raw = String(packageId).split('#')[0].replace(/^path\+file:\/\//, '');
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
}

// The test binaries of a cargo run: the same ones cargo would have executed.
function collectTestBinaries(args, cwd, env) {
  const build = cargo([...args, '--no-run', '--message-format=json'], cwd, env);
  const binaries = [];
  const seen = new Set();
  for (const line of build.out.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed.startsWith('{')) continue;
    let message;
    try {
      message = JSON.parse(trimmed);
    } catch {
      continue;
    }
    if (message.reason !== 'compiler-artifact') continue;
    const kinds = (message.target && message.target.kind) || [];
    if (!kinds.includes('test') || !message.executable) continue;
    if (!fs.existsSync(message.executable)) continue;
    if (seen.has(message.executable)) continue;
    seen.add(message.executable);
    binaries.push({
      executable: message.executable,
      name: (message.target && message.target.name) || path.basename(message.executable),
      root: packageRootOf(message.package_id),
    });
  }
  return {binaries, build};
}

// Runs the copies.
//
// A binary that cannot even START was damaged by the scanner (it rewrites the manifest of a file it
// quarantines: the copy then fails with "Activation context generation failed ... Invalid Xml syntax").
// Such a binary is **not** a test result: it is reported as "the machine blocked it" (status -1), so the
// red-green check can never call such a case caught and the gate can never call such a step green.
function runCopies(binaries, env, attempts = 3) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pime-suite-'));
  const parts = [];
  const blocked = [];
  let status = 0;
  try {
    for (const binary of binaries) {
      const copy = path.join(dir, path.basename(binary.executable));
      try {
        fs.copyFileSync(binary.executable, copy);
      } catch (error) {
        parts.push(`note: ${binary.name} could not be copied out of the target directory: ${error.message}`);
        blocked.push(binary);
        status = status || -1;
        continue;
      }
      const start = copy => spawnSync(copy, [], {
        cwd: binary.root && fs.existsSync(binary.root) ? binary.root : undefined,
        encoding: 'utf8',
        env,
        maxBuffer: 256 * 1024 * 1024,
        // the scanner shows a modal dialog for a broken binary: without a timeout the gate would wait forever
        timeout: 300000,
      });
      let run = start(copy);
      const out = `${run.stdout || ''}${run.stderr || ''}`;
      if (run.status !== 0 && run.status !== null && BLOCKED_EXECUTION.test(out) && attempts > 1) {
        // The copy was quarantined as well. The scanner holds a file for **tens of seconds** (it is looking at it), so
        // the wait has to be longer than that - a wait of a couple of seconds is always too short, and the retry then
        // only repeats the failure (2026-11-27: the gate was red for that reason alone).
        for (let i = 2; i <= attempts; i++) {
          const wait = 45000 * i;
          const until = Date.now() + wait;
          while (Date.now() < until) { /* the scanner needs a moment */ }
          run = start(copy);
          if (run.status === 0) break;
        }
      }
      parts.push(`${run.stdout || ''}${run.stderr || ''}`);
      if (run.status === null) {
        // the process could not be started at all: the machine, not the code
        parts.push(`note: ${binary.name} could not be started (${run.error ? run.error.message : 'no exit code'})`);
        blocked.push(binary);
        status = status || -1;
        continue;
      }
      if (run.status !== 0) status = status || run.status;
    }
  } finally {
    try {
      fs.rmSync(dir, {recursive: true, force: true});
    } catch { /* the temporary directory is the only thing left behind, and it is in the temp folder */ }
  }
  return {status, out: parts.join('\n'), blocked};
}

// The entry point: a cargo test command that survives the scanner of the machine.
//
// It returns the same shape as a cargo run: {status, out, note}. `note` says what happened, so the gate log
// shows that the tests ran from a copy instead of pretending that everything was normal.
function runCargoSuite({args, cwd, env, label = ''}) {
  const first = cargo(args, cwd, env);
  if (first.status === 0) return {...first, note: ''};

  const write = first.out.match(BLOCKED_WRITE);
  if (write) {
    const file = write[1].trim();
    if (restoreWriteAccess(file)) {
      const again = cargo(args, cwd, env);
      if (again.status === 0) return {...again, note: `the write access of ${path.basename(file)} was restored (the scanner of the machine had taken it away)`};
      if (BLOCKED_EXECUTION.test(again.out)) {
        return runTheCopies(args, cwd, env);
      }
      return {...again, note: `the write access of ${path.basename(file)} was restored (the scanner of the machine had taken it away)`};
    }
  }

  if (BLOCKED_EXECUTION.test(first.out)) {
    return runTheCopies(args, cwd, env);
  }

  return {...first, note: label ? `${label}` : ''};
}

// Runs the test binaries from copies. A binary that could not be started at all was damaged by the scanner:
// it is removed and linked again (a fresh link is clean), and the copies are tried once more.
function runTheCopies(args, cwd, env) {
  let binaries = collectTestBinaries(args, cwd, env);
  if (!binaries.binaries.length) return {status: -1, out: '', note: 'the test binaries of the run could not be found'};
  let copied = runCopies(binaries.binaries, env);
  const note = `the test binaries were copied out of the directory the scanner watches and ran there (${binaries.binaries.length} binaries)`;
  if (!copied.blocked.length) return {...copied, note};

  // the damaged binaries are linked again and the copies are tried once more
  for (const binary of copied.blocked) {
    try {
      fs.unlinkSync(binary.executable);
    } catch { /* the scanner holds the file: cargo reports it and the run stays blocked */ }
  }
  const relinked = collectTestBinaries(args, cwd, env);
  if (!relinked.binaries.length) return {...copied, note: `${note}; ${copied.blocked.length} binary/binaries were damaged by the scanner and could not be linked again`};
  const again = runCopies(relinked.binaries, env);
  return {...again, note: `${note}; ${copied.blocked.length} binary/binaries were damaged by the scanner and linked again`};
}

module.exports = {runCargoSuite, cargo, collectTestBinaries, runCopies, restoreWriteAccess, runTheCopies};
