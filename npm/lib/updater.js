const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const os = require('os');

const GITHUB_REPO = 'geoffsee/muxox';

function getPlatformInfo() {
  const platform = os.platform();
  const arch = os.arch();

  console.log(`Detected platform: ${platform} arch: ${arch}`);

  // Map Node.js platform/arch to Rust target triples
  const platformMap = {
    'darwin-x64': { target: 'x86_64-apple-darwin', ext: 'tar.gz' },
    'darwin-arm64': { target: 'aarch64-apple-darwin', ext: 'tar.gz' },
    'linux-x64': { target: 'x86_64-unknown-linux-gnu', ext: 'tar.gz' },
    'win32-x64': { target: 'x86_64-pc-windows-msvc', ext: 'exe.zip' }
  };

  const key = `${platform}-${arch}`;
  const info = platformMap[key];

  if (!info) {
    throw new Error(`Unsupported platform: ${platform}-${arch}`);
  }

  return info;
}

function httpsGet(url) {
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'User-Agent': 'muxox-updater' } }, (response) => {
      if (response.statusCode === 302 || response.statusCode === 301) {
        // Follow redirect
        httpsGet(response.headers.location).then(resolve).catch(reject);
      } else if (response.statusCode === 200) {
        let data = '';
        response.on('data', chunk => data += chunk);
        response.on('end', () => resolve({ statusCode: response.statusCode, data }));
      } else {
        resolve({ statusCode: response.statusCode, data: null });
      }
    }).on('error', reject);
  });
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    
    https.get(url, (response) => {
      if (response.statusCode === 302 || response.statusCode === 301) {
        // Follow redirect
        https.get(response.headers.location, (redirectResponse) => {
          if (redirectResponse.statusCode !== 200) {
            reject(new Error(`Failed to download: HTTP ${redirectResponse.statusCode}`));
            return;
          }
          redirectResponse.pipe(file);
          file.on('finish', () => {
            file.close();
            resolve();
          });
        }).on('error', reject);
      } else if (response.statusCode === 200) {
        response.pipe(file);
        file.on('finish', () => {
          file.close();
          resolve();
        });
      } else {
        reject(new Error(`Failed to download: HTTP ${response.statusCode}`));
      }
    }).on('error', (err) => {
      fs.unlink(dest, () => {});
      reject(err);
    });

    file.on('error', (err) => {
      fs.unlink(dest, () => {});
      reject(err);
    });
  });
}

function extractTarGz(archivePath, destDir) {
  try {
    execSync(`tar -xzf "${archivePath}" -C "${destDir}"`, { stdio: 'pipe' });
  } catch (error) {
    throw new Error(`Failed to extract tar.gz: ${error.message}`);
  }
}

function extractZip(archivePath, destDir) {
  try {
    if (process.platform === 'win32') {
      execSync(`powershell -command "Expand-Archive -Path '${archivePath}' -DestinationPath '${destDir}' -Force"`, { stdio: 'pipe' });
    } else {
      execSync(`unzip -o "${archivePath}" -d "${destDir}"`, { stdio: 'pipe' });
    }
  } catch (error) {
    throw new Error(`Failed to extract zip: ${error.message}`);
  }
}

async function getLatestVersion() {
  try {
    const response = await httpsGet(`https://api.github.com/repos/${GITHUB_REPO}/releases/latest`);
    
    if (response.statusCode === 200) {
      const release = JSON.parse(response.data);
      return release.tag_name.replace(/^v/, ''); // Remove 'v' prefix
    }
    return null;
  } catch (error) {
    return null;
  }
}

function compareVersions(v1, v2) {
  const parts1 = v1.split('.').map(Number);
  const parts2 = v2.split('.').map(Number);
  
  for (let i = 0; i < Math.max(parts1.length, parts2.length); i++) {
    const part1 = parts1[i] || 0;
    const part2 = parts2[i] || 0;
    
    if (part1 > part2) return 1;
    if (part1 < part2) return -1;
  }
  
  return 0;
}

async function updateBinary(version, binDir) {
  try {
    const platformInfo = getPlatformInfo();
    
    // Construct download URL
    const archiveName = platformInfo.ext === 'exe.zip' 
      ? `muxox-${platformInfo.target}.exe.zip`
      : `muxox-${platformInfo.target}.tar.gz`;
    
    const downloadUrl = `https://github.com/${GITHUB_REPO}/releases/download/v${version}/${archiveName}`;
    const archivePath = path.join(binDir, archiveName);

    // Download the archive
    await download(downloadUrl, archivePath);

    // Extract the archive
    if (platformInfo.ext === 'tar.gz') {
      extractTarGz(archivePath, binDir);
    } else {
      extractZip(archivePath, binDir);
    }

    // Clean up archive
    fs.unlinkSync(archivePath);

    // Make binary executable on Unix-like systems
    const binaryName = process.platform === 'win32' ? 'muxox.exe' : 'muxox';
    const binaryPath = path.join(binDir, binaryName);
    
    if (process.platform !== 'win32') {
      fs.chmodSync(binaryPath, 0o755);
    }

    return true;
  } catch (error) {
    throw error;
  }
}

module.exports = {
  getPlatformInfo,
  download,
  extractTarGz,
  extractZip,
  getLatestVersion,
  compareVersions,
  updateBinary,
  GITHUB_REPO
};
