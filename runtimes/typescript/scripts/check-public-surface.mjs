import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const distDir = resolve(packageRoot, "dist");
const goldenPath = resolve(packageRoot, "etc", "public-surface.d.ts");

async function declarationFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = await Promise.all(
    entries.map(async (entry) => {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) return declarationFiles(path);
      return entry.isFile() && entry.name.endsWith(".d.ts") ? [path] : [];
    }),
  );
  return files.flat().sort();
}

async function renderSurface() {
  const files = await declarationFiles(distDir);
  if (files.length === 0) {
    throw new Error("no declaration files found; run `npm run build` first");
  }

  const sections = await Promise.all(
    files.map(async (path) => {
      const name = relative(distDir, path).replaceAll("\\", "/");
      const declaration = (await readFile(path, "utf8"))
        .replace(/^\/\/# sourceMappingURL=.*$/gm, "")
        .trimEnd();
      return `// --- ${name} ---\n${declaration}`;
    }),
  );

  return `${sections.join("\n\n")}\n`;
}

const actual = await renderSurface();

if (process.argv.includes("--update")) {
  await mkdir(dirname(goldenPath), { recursive: true });
  await writeFile(goldenPath, actual);
  console.log(`updated ${relative(packageRoot, goldenPath)}`);
} else {
  let expected;
  try {
    expected = await readFile(goldenPath, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") {
      throw new Error(
        "public-surface golden is missing; run `npm run build` and then `node scripts/check-public-surface.mjs --update`",
      );
    }
    throw error;
  }

  if (actual !== expected) {
    throw new Error(
      "TypeScript public declarations changed; review the diff and run `node scripts/check-public-surface.mjs --update` if intentional",
    );
  }

  console.log("TypeScript public declarations match the committed golden");
}
