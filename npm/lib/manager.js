'use strict';

// Installation ownership is recorded by postinstall. Never guess npm from PATH.
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn, spawnSync } = require('child_process');
const PACKAGE_ROOT = path.resolve(__dirname, '..');
const RECEIPT = '.muxox-install.json';
const HANDOFF_EXIT = 75;
const real = p => fs.realpathSync(p);
const readJson = p => JSON.parse(fs.readFileSync(p, 'utf8'));
const inside = (root, p) => { const rel = path.relative(root, p); return rel === '' || (!rel.startsWith('..' + path.sep) && rel !== '..' && !path.isAbsolute(rel)); };
const temporary = p => /(?:^|[\\/])(?:_npx|dlx|bunx)(?:[\\/]|$)|[\\/](?:bunx|dlx)-/.test(p);
const stable = v => /^\d+\.\d+\.\d+(?:\+[0-9A-Za-z.-]+)?$/.test(v);

function launch(manager, args) {
  return { file: manager.runtime || manager.launcher, args: manager.runtime ? [manager.launcher, ...args] : args };
}
function querySync(manager, args, cwd) {
  const command = launch(manager, args);
  const result = spawnSync(command.file, command.args, { cwd, encoding: 'utf8', timeout: 4000, windowsHide: true });
  if (result.error || result.status !== 0) throw new Error(`Cannot query ${manager.name}: ${result.error ? result.error.message : result.stderr}`);
  return result.stdout.trim();
}
function managerFromEnv(env) {
  const match = /^(npm|pnpm|yarn|bun)\/(\d+)(?:\.[^ ]*)?/.exec(env.npm_config_user_agent || '');
  if (!match || !env.npm_execpath) throw new Error('Cannot identify the installing package manager. Reinstall with npm, pnpm, Yarn, or Bun.');
  const launcher = real(env.npm_execpath);
  // Lifecycle variables normally point at a JS entry point, even on Windows.
  const fd = fs.openSync(launcher, 'r');
  const header = Buffer.alloc(200);
  try { fs.readSync(fd, header, 0, header.length, 0); } finally { fs.closeSync(fd); }
  const script = /\.[cm]?js$/.test(launcher) || /^#![^\n]*\bnode\b/.test(header.toString());
  if (/\.(cmd|bat)$/i.test(launcher)) throw new Error('A JavaScript or native package-manager launcher is required.');
  return { name: match[1], major: Number(match[2]), launcher, runtime: script ? real(env.npm_node_execpath || process.execPath) : null };
}
function resolvesPackage(owner, packageRoot) {
  try { return real(require.resolve('muxox/package.json', { paths: [owner] })) === path.join(packageRoot, 'package.json'); }
  catch (_) {
    // Yarn PnP exposes the same resolution without requiring a node_modules tree.
    let dir = owner;
    while (true) {
      const pnp = path.join(dir, '.pnp.cjs');
      if (fs.existsSync(pnp)) {
        try { return real(require(pnp).resolveRequest('muxox/package.json', path.join(owner, 'package.json'))) === path.join(packageRoot, 'package.json'); }
        catch (_) { return false; }
      }
      if (path.dirname(dir) === dir) return false;
      dir = path.dirname(dir);
    }
  }
}
function directDependency(owner, packageRoot) {
  try {
    const manifest = readJson(path.join(owner, 'package.json'));
    const section = ['dependencies', 'devDependencies', 'optionalDependencies'].find(key => manifest[key] && Object.prototype.hasOwnProperty.call(manifest[key], 'muxox'));
    return section && resolvesPackage(owner, packageRoot) ? section : null;
  } catch (_) { return null; }
}
function localRoot(packageRoot) {
  const parts = packageRoot.split(path.sep);
  const index = parts.findIndex(part => part === 'node_modules' || part === '.yarn');
  if (index < 0) throw new Error('Cannot locate the project owning this installation.');
  return parts.slice(0, index).join(path.sep) || path.parse(packageRoot).root;
}
function recordReceipt(packageRoot = PACKAGE_ROOT, env = process.env) {
  packageRoot = real(packageRoot);
  try { fs.unlinkSync(path.join(packageRoot, RECEIPT)); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  // A missing receipt is safe: startup checks stay quiet and explicit updates explain why.
  const receipt = { schema: 1, packageRoot, manager: managerFromEnv(env), scope: 'local', root: null, installCwd: env.INIT_CWD || null };
  if (temporary(packageRoot)) { receipt.scope = 'temporary'; }
  else {
    const manager = receipt.manager;
    let globalRoot;
    try {
      if (manager.name === 'npm' || manager.name === 'pnpm') {
        const modules = querySync(manager, ['root', '--global'], packageRoot);
        if (real(path.join(modules, 'muxox')) === packageRoot) {
          globalRoot = path.dirname(modules);
          if (manager.name === 'npm') {
            // npm's prefix is above lib/node_modules on Unix and node_modules on Windows.
            globalRoot = path.basename(globalRoot) === 'lib' ? path.dirname(globalRoot) : globalRoot;
          }
        }
      } else if (manager.name === 'yarn' && manager.major === 1) {
        const root = querySync(manager, ['global', 'dir'], packageRoot);
        if (real(path.join(root, 'node_modules', 'muxox')) === packageRoot) {
          globalRoot = root;
          receipt.globalBin = querySync(manager, ['global', 'bin'], packageRoot);
        }
      } else if (manager.name === 'bun') {
        const root = env.BUN_INSTALL_GLOBAL_DIR || path.join(env.BUN_INSTALL || path.join(os.homedir(), '.bun'), 'install', 'global');
        if (real(path.join(root, 'node_modules', 'muxox')) === packageRoot) globalRoot = root;
      }
    } catch (_) { /* Local installs don't need a global directory. */ }
    // An explicit npm prefix may differ from the user's default prefix.
    if (!globalRoot && manager.name === 'npm' && env.npm_config_global === 'true' && env.npm_config_prefix) {
      const prefix = real(env.npm_config_prefix);
      const modules = process.platform === 'win32' ? path.join(prefix, 'node_modules') : path.join(prefix, 'lib', 'node_modules');
      if (real(path.join(modules, 'muxox')) === packageRoot) globalRoot = prefix;
    }
    if (globalRoot) { receipt.scope = 'global'; receipt.root = real(globalRoot); }
    else {
      if (env.npm_config_global === 'true') throw new Error('Cannot verify this global installation. Use its original package-manager command.');
      receipt.root = real(localRoot(packageRoot));
    }
  }
  fs.writeFileSync(path.join(packageRoot, RECEIPT), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 });
  return receipt;
}
function loadReceipt(packageRoot = PACKAGE_ROOT) {
  packageRoot = real(packageRoot);
  let receipt;
  try { receipt = readJson(path.join(packageRoot, RECEIPT)); }
  catch (_) { throw new Error('Installation receipt is missing. Reinstall muxox with its original package manager to enable updates.'); }
  if (receipt.schema !== 1 || receipt.packageRoot !== packageRoot || !receipt.manager || !['npm', 'pnpm', 'yarn', 'bun'].includes(receipt.manager.name)) {
    throw new Error('Installation receipt is stale or unsupported. Reinstall with the original package manager.');
  }
  if (receipt.scope === 'temporary' || temporary(packageRoot)) throw new Error('This is a temporary package-runner installation. Run the original npx/dlx/bunx command with muxox@latest.');
  if (!['global', 'local'].includes(receipt.scope) || !receipt.root || !path.isAbsolute(receipt.root)) throw new Error('Installation scope is unknown. Use the original package manager.');
  if (!fs.existsSync(receipt.manager.launcher) || (receipt.manager.runtime && !fs.existsSync(receipt.manager.runtime))) throw new Error('The installing package manager is no longer available. Reinstall with your chosen manager.');
  return receipt;
}
function ownerContext(receipt, invocationCwd = process.env.MUXOX_INVOKE_CWD || process.cwd()) {
  if (receipt.scope === 'global') return { cwd: receipt.root, section: null };
  // Resolve the nearest direct owner. Never use a different project just because
  // the command was invoked there, or update every workspace through `yarn up`.
  for (const start of [invocationCwd, receipt.installCwd, receipt.root]) {
    if (!start || !inside(receipt.root, path.resolve(start))) continue;
    let cwd = path.resolve(start);
    while (inside(receipt.root, cwd)) {
      const section = fs.existsSync(cwd) && inside(receipt.root, real(cwd)) && directDependency(cwd, receipt.packageRoot);
      if (section) return { cwd, section };
      if (cwd === receipt.root) break;
      cwd = path.dirname(cwd);
    }
  }
  throw new Error('Cannot identify a direct project dependency on this installation. Run muxox update from the owning workspace, or update it with its package manager.');
}
function checkArgs(receipt) {
  const { name, major } = receipt.manager;
  if (name === 'yarn') return major === 1 ? ['info', 'muxox', 'version', '--json'] : ['npm', 'info', 'muxox', '--fields', 'version', '--json'];
  if (name === 'bun') return ['info', 'muxox', 'version', '--json'];
  return ['view', 'muxox', 'version', '--json'];
}
function yarnPrefix(receipt) {
  if (!receipt.globalBin || path.basename(receipt.globalBin) !== 'bin') throw new Error('Cannot preserve the Yarn global binary directory. Use yarn global upgrade manually.');
  return path.dirname(receipt.globalBin);
}
function updateArgs(receipt, version, section) {
  if (!stable(version)) throw new Error('Invalid stable update version.');
  const spec = `muxox@${version}`;
  const global = receipt.scope === 'global';
  switch (receipt.manager.name) {
    case 'npm': return global ? ['install', '--global', '--prefix', receipt.root, spec] : ['install', spec];
    case 'pnpm': return ['update', ...(global ? ['--global'] : []), spec];
    case 'yarn':
      if (receipt.manager.major === 1) return global ? ['global', 'upgrade', '--global-folder', receipt.root, '--prefix', yarnPrefix(receipt), spec] : ['upgrade', spec];
      if (global) throw new Error('Modern Yarn does not support global upgrades.');
      return ['add', ...(section === 'optionalDependencies' ? ['--optional'] : []), spec];
    case 'bun': return ['update', ...(global ? ['--global'] : []), spec, '--latest'];
    default: throw new Error('Unsupported package manager.');
  }
}
function parseVersion(output, manager) {
  let value;
  if (manager.name === 'yarn' && manager.major === 1) {
    for (const line of output.trim().split(/\r?\n/)) {
      const item = JSON.parse(line);
      if (item.type === 'inspect') value = item.data;
    }
  } else {
    value = JSON.parse(output);
    if (value && typeof value === 'object') value = value.version;
  }
  if (typeof value !== 'string' || !stable(value)) throw new Error('Registry did not return a stable muxox version.');
  return value;
}
function run(manager, args, cwd, inherit = false) {
  const command = launch(manager, args);
  return new Promise((resolve, reject) => {
    const child = spawn(command.file, command.args, { cwd, stdio: inherit ? 'inherit' : ['ignore', 'pipe', 'pipe'], windowsHide: !inherit });
    let stdout = '', stderr = '';
    if (!inherit) {
      child.stdout.on('data', data => { stdout += data; });
      child.stderr.on('data', data => { stderr += data; });
    }
    const forward = signal => { if (!child.killed) child.kill(signal); };
    const interrupt = () => forward('SIGINT'), terminate = () => forward('SIGTERM');
    process.on('SIGINT', interrupt); process.on('SIGTERM', terminate);
    child.once('error', reject);
    child.once('close', (code, signal) => {
      process.removeListener('SIGINT', interrupt); process.removeListener('SIGTERM', terminate);
      if (signal) { const error = new Error(`${manager.name} terminated by ${signal}`); error.signal = signal; reject(error); }
      else if (code !== 0) { const error = new Error(`${manager.name} failed (${code})${stderr ? ': ' + stderr.trim() : ''}`); error.exitCode = code || 1; reject(error); }
      else resolve(stdout);
    });
  });
}
function verifyGlobal(receipt) {
  if (receipt.scope !== 'global') return;
  const { manager, packageRoot, root } = receipt;
  let modules;
  if (manager.name === 'npm') modules = querySync(manager, ['root', '--global', '--prefix', root], root);
  if (manager.name === 'pnpm') modules = querySync(manager, ['root', '--global'], root);
  if (manager.name === 'yarn') modules = path.join(root, 'node_modules');
  if (manager.name === 'bun') {
    const defaultRoot = process.env.BUN_INSTALL_GLOBAL_DIR || path.join(process.env.BUN_INSTALL || path.join(os.homedir(), '.bun'), 'install', 'global');
    if (real(defaultRoot) !== root) throw new Error('Bun global directory changed. Use the original Bun installation.');
    modules = path.join(root, 'node_modules');
  }
  if (!modules || real(path.join(modules, 'muxox')) !== packageRoot) throw new Error('The package manager now targets a different global installation. Use the original manager configuration.');
}
async function check(packageRoot = PACKAGE_ROOT) {
  const receipt = loadReceipt(packageRoot);
  const context = ownerContext(receipt);
  verifyGlobal(receipt);
  const output = await run(receipt.manager, checkArgs(receipt), context.cwd);
  return { version: parseVersion(output, receipt.manager) };
}
async function update(request, packageRoot = PACKAGE_ROOT, nativeVersion) {
  if (!request || request.schema !== 1 || !stable(request.version) || typeof request.current_version !== 'string') throw new Error('Invalid update handoff.');
  const receipt = loadReceipt(packageRoot);
  const context = ownerContext(receipt);
  verifyGlobal(receipt);
  if (nativeVersion !== request.current_version) throw new Error('The installation changed during the update. Run muxox update again.');
  // Version precedence was checked by the native client against the actual binary.
  const args = updateArgs(receipt, request.version, context.section);
  console.error(`Updating Muxox to ${request.version} with ${receipt.manager.name}...`);
  await run(receipt.manager, args, context.cwd, true);
}
module.exports = { HANDOFF_EXIT, recordReceipt, loadReceipt, ownerContext, checkArgs, updateArgs, parseVersion, check, update, launch, temporary, stable };
if (require.main === module) {
  if (process.argv[2] !== 'check') { console.error('Use the muxox launcher to update.'); process.exitCode = 1; }
  else check().then(result => process.stdout.write(JSON.stringify(result) + '\n')).catch(error => { console.error(error.message); process.exitCode = 1; });
}
