// Downloads the prebuilt r3v3rs3-mcp binary of this package's version from the GitHub
// releases of r3v3rs3 and verifies it against the signed SHA256SUMS of the release.
"use strict";

const { createHash } = require("node:crypto");
const { chmodSync, createWriteStream, mkdirSync, rmSync, statSync } = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { pipeline } = require("node:stream/promises");

const REPO = "KilimcininKorOglu/r3v3rs3";
const VERSION = require("./package.json").version;
const USER_AGENT = "r3v3rs3-mcp npm installer";

// The musl build is static, so one archive per architecture runs on every Linux.
const TARGETS = {
  "linux-x64": "r3v3rs3-mcp-x86_64-unknown-linux-musl.tar.gz",
  "linux-arm64": "r3v3rs3-mcp-aarch64-unknown-linux-musl.tar.gz",
};

function targetOf(platform, arch) {
  const target = TARGETS[`${platform}-${arch}`];
  if (!target) {
    throw new Error(`r3v3rs3-mcp ships no binary for ${platform}/${arch}`);
  }
  return target;
}

async function download(url) {
  const response = await fetch(url, { headers: { "user-agent": USER_AGENT } });
  if (!response.ok) {
    throw new Error(`${url} answered with ${response.status}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

function digestOf(sums, asset) {
  for (const line of sums.split("\n")) {
    const [digest, name] = line.trim().split(/\s+/);
    if (name && name.replace(/^\.\//, "") === asset) {
      return digest;
    }
  }
  throw new Error(`${asset} is not in the checksums of the release`);
}

async function main() {
  const asset = targetOf(process.platform, process.arch);
  const base = `https://github.com/${REPO}/releases/download/v${VERSION}`;
  process.stdout.write(`Downloading r3v3rs3-mcp ${VERSION}...\n`);
  const sums = (await download(`${base}/SHA256SUMS`)).toString("utf8");
  const archive = await download(`${base}/${asset}`);
  if (sha256(archive) !== digestOf(sums, asset)) {
    throw new Error(`the sha256 digest of ${asset} does not match the checksums of the release`);
  }
  const destination = path.join(__dirname, "bin");
  mkdirSync(destination, { recursive: true });
  const archivePath = path.join(destination, asset);
  await pipeline(require("node:stream").Readable.from(archive), createWriteStream(archivePath));
  // Node has no tar reader of its own; every system that runs npm ships tar.
  execFileSync("tar", ["-xzf", archivePath, "r3v3rs3-mcp"], { cwd: destination });
  rmSync(archivePath);
  const binary = path.join(destination, "r3v3rs3-mcp");
  chmodSync(binary, 0o755);
  statSync(binary);
  process.stdout.write(`r3v3rs3-mcp ${VERSION} is installed.\n`);
}

main().catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exit(1);
});
