import { createHash } from "node:crypto";
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { compile } from "json-schema-to-typescript";

const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(webRoot, "../..");
const outputDirectory = resolve(webRoot, "src/api/generated");

rmSync(outputDirectory, { recursive: true, force: true });

const exported = spawnSync(
  "cargo",
  [
    "run",
    "--manifest-path",
    resolve(repositoryRoot, "Cargo.toml"),
    "-p",
    "kuku-server",
    "--bin",
    "export-web-contract",
    "--",
    outputDirectory,
  ],
  { cwd: repositoryRoot, stdio: "inherit" },
);

if (exported.error) {
  throw exported.error;
}
if (exported.status !== 0) {
  process.exit(exported.status ?? 1);
}

const schema = JSON.parse(
  readFileSync(resolve(outputDirectory, "schema.json"), "utf8"),
);
const declarations = await compile(schema, "WebApiContract", {
  bannerComment:
    "/** Generated from the server-owned API schema. Do not edit. */",
  unknownAny: true,
  unreachableDefinitions: true,
});
writeFileSync(resolve(outputDirectory, "index.ts"), declarations);

const manifestInputs = JSON.parse(
  readFileSync(resolve(outputDirectory, "manifest-inputs.json"), "utf8"),
);
const generatedFiles = [
  ...manifestInputs,
  "index.ts",
  "manifest-inputs.json",
].sort();
const hashes = Object.fromEntries(
  generatedFiles.map((relativePath) => {
    const contents = readFileSync(resolve(outputDirectory, relativePath));
    const digest = createHash("sha256").update(contents).digest("hex");
    return [relativePath, digest];
  }),
);
const manifest = { algorithm: "sha256", files: hashes };
writeFileSync(
  resolve(outputDirectory, "manifest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
