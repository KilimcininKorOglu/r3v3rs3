// Runs the downloaded r3v3rs3-mcp binary with the arguments of this command.
"use strict";

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const binary = path.join(__dirname, "r3v3rs3-mcp");
const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  process.stderr.write(
    `the r3v3rs3-mcp binary did not run: ${result.error.message}\n` +
      "run `npm rebuild -g r3v3rs3-mcp` to download it again\n",
  );
  process.exit(1);
}
process.exit(result.status ?? 1);
