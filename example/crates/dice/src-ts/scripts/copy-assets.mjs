import { cp, mkdir } from "node:fs/promises";

await mkdir("build/example", { recursive: true });
await cp("example/native_bg.wasm", "build/example/native_bg.wasm");
