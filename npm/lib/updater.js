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
    'linux-arm64': { target: 'aarch64-unknown-linux-gnu', ext: 'tar.gz' },
    'win32-x64': { target: 'x86_64-pc-windows-msvc', ext: 'exe.zip' }
  };

  const key = `${platform}-${arch}`;
  const info = platformMap[key];

  if (!info) {
    throw new Error(`Unsupported platform: ${platform}-${arch}`);
  }

  return info;
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
  updateBinary,
  GITHUB_REPO
};
