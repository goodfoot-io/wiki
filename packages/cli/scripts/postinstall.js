#!/usr/bin/env node

"use strict";

const fs = require("fs");
const path = require("path");

const PLATFORM_MAP = {
  linux: "linux",
  darwin: "darwin",
  win32: "win32",
};

const ARCH_MAP = {
  x64: "x64",
  arm64: "arm64",
};

function main() {
  const platform = PLATFORM_MAP[process.platform];
  const arch = ARCH_MAP[process.arch];

  if (!platform || !arch) {
    console.log(
      `@goodfoot/wiki: No prebuilt binary available for ${process.platform}-${process.arch}. Build from source with "cargo build --release".`
    );
    process.exit(0);
  }

  const packageName = `@goodfoot/wiki-${platform}-${arch}`;
  const binaryName = process.platform === "win32" ? "wiki.exe" : "wiki";

  let packageDir;
  try {
    packageDir = path.dirname(require.resolve(`${packageName}/package.json`));
  } catch {
    // Platform package not installed -- user may have built from source
    console.log(
      `@goodfoot/wiki: Optional package ${packageName} not found. Skipping binary setup.`
    );
    process.exit(0);
  }

  const sourceBinary = path.join(packageDir, "bin", binaryName);
  if (!fs.existsSync(sourceBinary)) {
    console.log(
      `@goodfoot/wiki: Binary not found in ${packageName}. The package may not have been published yet.`
    );
    process.exit(0);
  }

  const targetDir = path.join(__dirname, "..", "bin");
  const targetBinary = path.join(targetDir, binaryName);

  fs.mkdirSync(targetDir, { recursive: true });

  // Remove existing binary if present
  try {
    fs.unlinkSync(targetBinary);
  } catch {
    // Ignore if it doesn't exist
  }

  // Try symlink first, fall back to copy
  try {
    fs.symlinkSync(sourceBinary, targetBinary);
  } catch {
    fs.copyFileSync(sourceBinary, targetBinary);
    fs.chmodSync(targetBinary, 0o755);
  }

  console.log(`@goodfoot/wiki: Installed ${binaryName} from ${packageName}`);
}

main();
