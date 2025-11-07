#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const { updateBinary, GITHUB_REPO } = require('./lib/updater');

const VERSION = require('./package.json').version;

async function install() {
  try {
    const binDir = path.join(__dirname, 'bin');
    
    // Create bin directory if it doesn't exist
    if (!fs.existsSync(binDir)) {
      fs.mkdirSync(binDir, { recursive: true });
    }

    console.log(`Installing muxox v${VERSION}...`);
    await updateBinary(VERSION, binDir);

    console.log('✓ muxox binary installed successfully');
  } catch (error) {
    console.error('Failed to install muxox binary:', error.message);
    console.error('\nYou can manually download the binary from:');
    console.error(`https://github.com/${GITHUB_REPO}/releases/tag/v${VERSION}`);
    process.exit(1);
  }
}

install();
