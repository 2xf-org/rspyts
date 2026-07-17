import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptRoot = fileURLToPath(new URL(".", import.meta.url));
const fixtureRoot = resolve(scriptRoot, "..");
const packageRoot = resolve(scriptRoot, "../../.rspyts/typescript");
const temporary = mkdtempSync(resolve(tmpdir(), "rspyts-packed-browser-"));
const consumer = resolve(temporary, "consumer");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

try {
  const packed = execFileSync(
    npm,
    ["pack", "--json", "--pack-destination", temporary, packageRoot],
    { encoding: "utf8" },
  );
  const [{ filename, files }] = JSON.parse(packed);
  const names = new Set(files.map(({ path }) => path));
  if (!names.has("native_bg.wasm") || !names.has("index.js") || !names.has("index.d.ts")) {
    throw new Error(`packed artifact is incomplete: ${[...names].sort().join(", ")}`);
  }

  const tarball = resolve(temporary, filename);
  const fixturePackage = JSON.parse(
    readFileSync(resolve(fixtureRoot, "package.json"), "utf8"),
  );
  const fixtureLock = JSON.parse(
    readFileSync(resolve(fixtureRoot, "package-lock.json"), "utf8"),
  );
  const exactDevDependencies = Object.fromEntries(
    Object.keys(fixturePackage.devDependencies).map((dependency) => {
      const version = fixtureLock.packages[`node_modules/${dependency}`]?.version;
      if (version === undefined) {
        throw new Error(`package-lock.json does not pin ${dependency}`);
      }
      return [dependency, version];
    }),
  );
  mkdirSync(consumer);
  cpSync(resolve(fixtureRoot, "tests"), resolve(consumer, "tests"), {
    recursive: true,
  });
  copyFileSync(
    resolve(fixtureRoot, "tsconfig.json"),
    resolve(consumer, "tsconfig.json"),
  );
  copyFileSync(
    resolve(fixtureRoot, "vitest.config.ts"),
    resolve(consumer, "vitest.config.ts"),
  );
  writeFileSync(
    resolve(consumer, "package.json"),
    `${JSON.stringify(
      {
        name: "rspyts-packed-acceptance-consumer",
        version: "0.0.0",
        private: true,
        type: "module",
        dependencies: {
          "@rspyts/acceptance": `file:${tarball}`,
        },
        devDependencies: exactDevDependencies,
      },
      null,
      2,
    )}\n`,
  );
  execFileSync(
    npm,
    ["install", "--package-lock=false", "--no-audit", "--no-fund"],
    { cwd: consumer, stdio: "inherit" },
  );

  const installed = JSON.parse(
    readFileSync(
      resolve(consumer, "node_modules/@rspyts/acceptance/package.json"),
      "utf8",
    ),
  );
  for (const dependency of [
    ...Object.keys(installed.dependencies ?? {}),
    ...Object.keys(installed.peerDependencies ?? {}),
  ]) {
    if (dependency === "rspyts") {
      throw new Error("packed consumer unexpectedly depends on an npm rspyts runtime");
    }
  }

  execFileSync(
    npm,
    ["exec", "--", "tsc", "--noEmit"],
    { cwd: consumer, stdio: "inherit" },
  );
  execFileSync(
    npm,
    ["exec", "--", "vitest", "run", "--browser.headless"],
    { cwd: consumer, stdio: "inherit" },
  );
} finally {
  rmSync(temporary, { force: true, recursive: true });
}
