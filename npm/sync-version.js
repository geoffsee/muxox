#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

// Read Cargo.toml to extract version
function getCargoVersion() {
  const cargoTomlPath = path.join(__dirname, '..', 'crates', 'muxox', 'Cargo.toml');
  const cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
  
  // Parse version from Cargo.toml
  const versionMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
  
  if (!versionMatch) {
    throw new Error('Could not find version in Cargo.toml');
  }
  
  return versionMatch[1];
}

// Update package.json with the version from Cargo.toml
function syncVersion() {
  try {
    const cargoVersion = getCargoVersion();
    const packageJsonPath = path.join(__dirname, 'package.json');
    
    // Read package.json
    const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));
    
    // Check if version needs updating
    if (packageJson.version === cargoVersion) {
      console.log(`✓ Versions already in sync: ${cargoVersion}`);
      return;
    }
    
    // Store old version before updating
    const oldVersion = packageJson.version;
    
    // Update version
    packageJson.version = cargoVersion;
    
    // Write back to package.json with proper formatting
    fs.writeFileSync(packageJsonPath, JSON.stringify(packageJson, null, 2) + '\n');
    
    console.log(`✓ Updated package.json version from ${oldVersion} to ${cargoVersion}`);
  } catch (error) {
    console.error('Error syncing version:', error.message);
    process.exit(1);
  }
}

// Run if executed directly
if (require.main === module) {
  syncVersion();
}

module.exports = { getCargoVersion, syncVersion };
