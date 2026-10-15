#!/usr/bin/env node
'use strict';

const { spawn, spawnSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');
// Load before the manager replaces this package on disk.
const { HANDOFF_EXIT, update } = require('../lib/manager');
const binaryPath = path.join(__dirname, process.platform === 'win32' ? 'muxox.exe' : 'muxox');
if (!fs.existsSync(binaryPath)) {
  console.error('Muxox binary not found. Reinstall with your original package manager.');
  process.exit(1);
}
const args = process.argv.slice(2);
const updating = args.includes('update');
const temp = updating ? fs.mkdtempSync(path.join(os.tmpdir(), 'muxox-update-')) : null;
const requestPath = temp ? path.join(temp, 'request.json') : null;
const env = { ...process.env, MUXOX_NODE_EXEC_PATH: process.execPath, MUXOX_INVOKE_CWD: process.cwd() };
delete env.MUXOX_UPDATE_REQUEST;
if (requestPath) env.MUXOX_UPDATE_REQUEST = requestPath;
const child = spawn(binaryPath, args, { stdio: 'inherit', env, windowsHide: false });
const interrupt = () => child.kill('SIGINT'), terminate = () => child.kill('SIGTERM');
process.on('SIGINT', interrupt); process.on('SIGTERM', terminate);
function cleanup() {
  process.removeListener('SIGINT', interrupt); process.removeListener('SIGTERM', terminate);
  if (temp) { try { fs.unlinkSync(requestPath); } catch (_) {} try { fs.rmdirSync(temp); } catch (_) {} }
}
child.on('error', error => { cleanup(); console.error('Failed to start muxox:', error.message); process.exitCode = 1; });
child.on('close', async (code, signal) => {
  process.removeListener('SIGINT', interrupt); process.removeListener('SIGTERM', terminate);
  try {
    if (code === HANDOFF_EXIT && updating && fs.existsSync(requestPath)) {
      const request = JSON.parse(fs.readFileSync(requestPath, 'utf8'));
      const result = spawnSync(binaryPath, ['--version'], { encoding: 'utf8', timeout: 5000 });
      if (result.error || result.status !== 0) throw new Error('Cannot verify the installed native executable.');
      const version = result.stdout.trim().replace(/^muxox /, '');
      await update(request, path.resolve(__dirname, '..'), version);
      process.exitCode = 0;
    } else if (signal) { cleanup(); process.kill(process.pid, signal); }
    else process.exitCode = code === null ? 1 : code;
  } catch (error) {
    console.error(error.message);
    if (error.signal) { cleanup(); process.kill(process.pid, error.signal); }
    else process.exitCode = error.exitCode || 1;
  }
  finally { cleanup(); }
});
