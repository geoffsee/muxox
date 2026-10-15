'use strict';
const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn, spawnSync } = require('child_process');
const manager = require('../lib/manager');
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'muxox-manager-test-')));
const write = (file, content) => { fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, typeof content === 'string' ? content : JSON.stringify(content)); };
const project = path.join(root, 'project with spaces');
const packageRoot = path.join(project, 'node_modules', 'muxox');
const log = path.join(root, 'calls.jsonl');
const launcher = path.join(root, 'manager.js');
let checks = 0;
function test(name, fn) { fn(); checks++; console.log(`ok - ${name}`); }
async function main() {
  write(path.join(project, 'package.json'), { devDependencies: { muxox: '^1.0.0' } });
  write(path.join(packageRoot, 'package.json'), { name: 'muxox', version: '1.0.0' });
  write(launcher, `const fs=require('fs'); const args=process.argv.slice(2);
    fs.appendFileSync(${JSON.stringify(log)}, JSON.stringify({args,cwd:process.cwd()})+'\\n');
    if(args[0]==='root') console.log(process.env.MUXOX_TEST_GLOBAL_MODULES || ${JSON.stringify(path.join(root, 'global', 'node_modules'))});
    else if(['view','info'].includes(args[0]) || args[0]==='npm') {
      if (process.env.MUXOX_TEST_QUERY_FAIL) process.exit(1);
      const respond=()=>{ if(process.env.MUXOX_TEST_QUERY_MARKER) fs.writeFileSync(process.env.MUXOX_TEST_QUERY_MARKER,'survived'); console.log(JSON.stringify(process.env.MUXOX_TEST_REGISTRY_VERSION || '99.0.0')); };
      setTimeout(respond, Number(process.env.MUXOX_TEST_QUERY_DELAY || 0));
    }
    else if (process.env.MUXOX_TEST_MANAGER_SIGNAL) process.kill(process.pid, 'SIGTERM');
    else process.exit(Number(process.env.MUXOX_TEST_MANAGER_EXIT || 0));`);
  const receipt = manager.recordReceipt(packageRoot, {
    npm_config_user_agent: 'npm/10.0.0 node/v20.0.0', npm_execpath: launcher, npm_node_execpath: process.execPath, INIT_CWD: project,
  });
  test('records verified local ownership and dependency section', () => {
    assert.equal(receipt.scope, 'local');
    assert.equal(manager.loadReceipt(packageRoot).manager.name, 'npm');
    assert.deepEqual(manager.ownerContext(receipt, project), { cwd: project, section: 'devDependencies' });
  });
  test('selects the invoking workspace without changing other workspaces', () => {
    const workspace = path.join(project, 'packages', 'app');
    write(path.join(workspace, 'package.json'), { dependencies: { muxox: '^1.0.0' } });
    assert.equal(manager.ownerContext(receipt, workspace).cwd, workspace);
  });
  test('refuses transitive-only ownership', () => {
    const original = fs.readFileSync(path.join(project, 'package.json'));
    write(path.join(project, 'package.json'), {});
    assert.throws(() => manager.ownerContext({ ...receipt, installCwd: project }, root), /direct project dependency/);
    fs.writeFileSync(path.join(project, 'package.json'), original);
  });
  test('selects manager-specific commands and preserves global npm prefix', () => {
    const r = name => ({ ...receipt, manager: { ...receipt.manager, name, major: name === 'yarn' ? 4 : 10 } });
    assert.deepEqual(manager.updateArgs(r('npm'), '2.0.0'), ['install', 'muxox@2.0.0']);
    assert.deepEqual(manager.updateArgs({ ...r('npm'), scope: 'global', root: '/custom prefix' }, '2.0.0'), ['install', '--global', '--prefix', '/custom prefix', 'muxox@2.0.0']);
    assert.deepEqual(manager.updateArgs(r('pnpm'), '2.0.0'), ['update', 'muxox@2.0.0']);
    assert.deepEqual(manager.updateArgs(r('yarn'), '2.0.0', 'devDependencies'), ['add', 'muxox@2.0.0']);
    assert.deepEqual(manager.updateArgs(r('bun'), '2.0.0'), ['update', 'muxox@2.0.0', '--latest']);
    const classic = { ...r('yarn'), manager: { ...r('yarn').manager, major: 1 } };
    assert.deepEqual(manager.updateArgs(classic, '2.0.0'), ['upgrade', 'muxox@2.0.0']);
    assert.deepEqual(manager.checkArgs(classic), ['info', 'muxox', 'version', '--json']);
    assert.deepEqual(manager.checkArgs(r('yarn')), ['npm', 'info', 'muxox', '--fields', 'version', '--json']);
    assert.throws(() => manager.updateArgs(r('npm'), '2.0.0;evil'), /Invalid/);
    assert.throws(() => manager.updateArgs(r('npm'), '2.0.0-rc.1'), /Invalid/);
  });
  test('registry responses include Yarn formats and reject malformed/prerelease versions', () => {
    assert.equal(manager.parseVersion('"2.0.0"', { name: 'npm' }), '2.0.0');
    assert.equal(manager.parseVersion('{"version":"2.0.0"}', { name: 'yarn', major: 4 }), '2.0.0');
    assert.equal(manager.parseVersion('{"type":"info","data":"ignored"}\n{"type":"inspect","data":"2.0.0"}', { name: 'yarn', major: 1 }), '2.0.0');
    assert.throws(() => manager.parseVersion('"2.0.0-rc.1"', { name: 'npm' }), /stable/);
    assert.throws(() => manager.parseVersion('garbage', { name: 'npm' }));
  });
  test('missing, stale and temporary receipts fail safely', () => {
    const receiptFile = path.join(packageRoot, '.muxox-install.json');
    write(receiptFile, { ...receipt, packageRoot: root });
    assert.throws(() => manager.loadReceipt(packageRoot), /stale/);
    write(receiptFile, { ...receipt, scope: 'temporary' });
    assert.throws(() => manager.loadReceipt(packageRoot), /temporary/);
    fs.unlinkSync(receiptFile);
    assert.throws(() => manager.loadReceipt(packageRoot), /missing/);
    write(receiptFile, receipt);
    assert(manager.temporary('/tmp/_npx/abcdef/node_modules/muxox'));
    assert(manager.temporary('/tmp/dlx/abcdef/node_modules/muxox'));
  });
  const before = fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8');
  assert.deepEqual(await manager.check(packageRoot), { version: '99.0.0' });
  assert.equal(fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8'), before);
  checks++; console.log('ok - registry check uses owner cwd and does not mutate package metadata');
  const calls = () => fs.readFileSync(log, 'utf8').trim().split('\n').map(JSON.parse);
  assert.deepEqual(calls().slice(-1)[0], { args: ['view', 'muxox', 'version', '--json'], cwd: project });
  await assert.rejects(() => manager.update({ schema: 1, version: '99.0.0', current_version: '1.0.0' }, packageRoot, '2.0.0'), /changed/);
  checks++;
  const globalPackage = path.join(root, 'global', 'node_modules', 'muxox');
  write(path.join(globalPackage, 'package.json'), { name: 'muxox', version: '1.0.0' });
  const globalReceipt = manager.recordReceipt(globalPackage, { npm_config_user_agent: 'npm/10.0.0', npm_execpath: launcher, npm_node_execpath: process.execPath });
  assert.equal(globalReceipt.scope, 'global');
  await manager.update({ schema: 1, current_version: '1.0.0', version: '2.0.0' }, globalPackage, '1.0.0');
  assert.deepEqual(calls().slice(-1)[0], { args: ['install', '--global', '--prefix', path.join(root, 'global'), 'muxox@2.0.0'], cwd: path.join(root, 'global') });
  checks++; console.log('ok - global updates retain prefix and never mutate the invoking project');
  manager.recordReceipt(globalPackage, { npm_config_user_agent: 'pnpm/10.0.0', npm_execpath: launcher, npm_node_execpath: process.execPath });
  process.env.MUXOX_TEST_GLOBAL_MODULES = path.join(root, 'different-global', 'node_modules');
  await assert.rejects(() => manager.update({ schema: 1, current_version: '1.0.0', version: '2.0.0' }, globalPackage, '1.0.0'));
  delete process.env.MUXOX_TEST_GLOBAL_MODULES;
  assert.equal(calls().slice(-1)[0].args[0], 'root');
  checks++; console.log('ok - changed pnpm global configuration cannot target another installation');
  const binary = process.env.MUXOX_TEST_BINARY;
  if (binary) {
    fs.mkdirSync(path.join(packageRoot, 'bin'), { recursive: true });
    fs.mkdirSync(path.join(packageRoot, 'lib'), { recursive: true });
    fs.copyFileSync(binary, path.join(packageRoot, 'bin', process.platform === 'win32' ? 'muxox.exe' : 'muxox'));
    fs.copyFileSync(path.join(__dirname, '../bin/muxox.js'), path.join(packageRoot, 'bin/muxox.js'));
    fs.copyFileSync(path.join(__dirname, '../lib/manager.js'), path.join(packageRoot, 'lib/manager.js'));
    const wrapper = path.join(packageRoot, 'bin/muxox.js');
    const run = (args, env = {}) => spawnSync(process.execPath, [wrapper, ...args], { cwd: project, encoding: 'utf8', timeout: 15000, env: { ...process.env, ...env } });
    test('ordinary wrapper launch does not query or overwrite the binary', () => {
      const count = calls().length;
      const result = run(['--version']);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /^muxox /);
      assert.equal(calls().length, count);
      assert.equal(fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8'), before);
    });
    test('native update hands off after exit and updates only owning project', () => {
      const result = run(['update']);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(calls().slice(-1)[0], { args: ['install', 'muxox@99.0.0'], cwd: project });
      assert(!fs.existsSync(path.join(project, 'muxox-debug.log')));
    });
    test('package-manager failures propagate from wrapper', () => {
      const result = run(['update'], { MUXOX_TEST_MANAGER_EXIT: '23' });
      assert.equal(result.status, 23, result.stderr);
    });
    if (process.platform !== 'win32') test('package-manager termination signals propagate from wrapper', () => {
      const result = run(['update'], { MUXOX_TEST_MANAGER_SIGNAL: '1' });
      assert.equal(result.signal, 'SIGTERM', result.stderr);
    });
    test('a registry version older than the actual binary cannot downgrade it', () => {
      const result = run(['update'], { MUXOX_TEST_REGISTRY_VERSION: '0.0.1' });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /up to date/);
      assert.equal(calls().slice(-1)[0].args[0], 'view');
    });
    // Use a unique name: stopped-service cleanup may match processes by name.
    write(path.join(project, 'muxox.toml'), `[[service]]\nname="${path.basename(root)}-service"\ncmd="echo startup-ready"\n`);
    async function rawLaunch(env, duration) {
      const start = Date.now();
      return new Promise((resolve, reject) => {
        const child = spawn(process.execPath, [wrapper, '--raw'], { cwd: project, env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'] });
        let stdout = '', stderr = '', readyMs;
        child.stdout.on('data', data => { stdout += data; if (stdout.includes('startup-ready') && readyMs === undefined) readyMs = Date.now() - start; });
        child.stderr.on('data', data => { stderr += data; });
        const timer = setTimeout(() => {
          if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F']);
          else child.kill('SIGTERM');
        }, duration);
        child.on('error', reject);
        child.on('close', () => { clearTimeout(timer); resolve({ stdout, stderr, readyMs }); });
      });
    }
    const online = await rawLaunch({}, 2000);
    assert(online.readyMs < 1500, JSON.stringify(online));
    assert.equal((online.stderr.match(/Muxox 99.0.0 is available/g) || []).length, 1, online.stderr);
    assert(!online.stdout.includes('is available'));
    checks++; console.log('ok - raw startup emits one notice on stderr without delaying services');
    const offline = await rawLaunch({ MUXOX_TEST_QUERY_FAIL: '1' }, 1000);
    assert(offline.readyMs < 900, JSON.stringify(offline));
    assert(!offline.stderr.includes('is available') && !offline.stderr.includes('failed'), offline.stderr);
    checks++; console.log('ok - failed registry checks stay quiet while services start');
    const marker = path.join(root, 'query-survived');
    const slow = await rawLaunch({ MUXOX_TEST_QUERY_DELAY: '4000', MUXOX_TEST_QUERY_MARKER: marker }, 4500);
    assert(slow.readyMs < 1500, JSON.stringify(slow));
    assert(!fs.existsSync(marker), 'timed-out package manager survived');
    assert(!slow.stderr.includes('is available'), slow.stderr);
    checks++; console.log('ok - timed-out checks kill package-manager descendants without delaying services');
    test('standalone override cannot bypass JavaScript ownership', () => {
      const count = calls().length;
      const result = run(['update', '--standalone']);
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /package-manager-owned/);
      assert.equal(calls().length, count);
    });
  } else console.log('Native wrapper integration skipped: set MUXOX_TEST_BINARY to a built muxox executable.');
  console.log(`${checks} package-manager checks passed`);
}
main().catch(error => { console.error(error); process.exitCode = 1; }).finally(() => fs.rmSync(root, { recursive: true, force: true }));
