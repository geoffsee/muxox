#!/usr/bin/env node

const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');
const { getLatestVersion, compareVersions, updateBinary } = require('../lib/updater');

// Determine the binary name based on platform
const binaryName = process.platform === 'win32' ? 'muxox.exe' : 'muxox';
const binaryPath = path.join(__dirname, binaryName);

// Check if binary exists
if (!fs.existsSync(binaryPath)) {
  console.error('Error: muxox binary not found.');
  console.error('Please try reinstalling the package: npm install muxox');
  process.exit(1);
}

async function checkForUpdates() {
  try {
    const packageJson = require('../package.json');
    const currentVersion = packageJson.version;
    
    // Check for latest version from GitHub
    const latestVersion = await getLatestVersion();
    
    if (latestVersion && compareVersions(latestVersion, currentVersion) > 0) {
      console.log(`\n🔄 New version available: ${latestVersion} (current: ${currentVersion})`);
      console.log('Downloading update...\n');
      
      const binDir = __dirname;
      await updateBinary(latestVersion, binDir);
      
      console.log(`✓ Updated to version ${latestVersion}\n`);
      
      // Update package.json version
      packageJson.version = latestVersion;
      fs.writeFileSync(
        path.join(__dirname, '..', 'package.json'),
        JSON.stringify(packageJson, null, 2) + '\n'
      );
    }
  } catch (error) {
    // Silently fail - don't block execution if update check fails
  }
}

function runBinary() {
  // Forward all arguments to the binary
  const args = process.argv.slice(2);

  // Spawn the binary process
  const child = spawn(binaryPath, args, {
    stdio: 'inherit',
    windowsHide: false
  });

  // Forward exit code
  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exit(code);
    }
  });

  // Handle errors
  child.on('error', (err) => {
    console.error('Failed to start muxox:', err.message);
    process.exit(1);
  });
}

// Check for updates then run binary
checkForUpdates().then(runBinary).catch(() => {
  // If update check fails, still run the binary
  runBinary();
});
