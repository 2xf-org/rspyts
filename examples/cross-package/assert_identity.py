"""Assert one canonical identity across owner and consumer host packages."""

from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).parent
OWNER = ROOT / "owner"
CONSUMER = ROOT / "consumer"


def read(path: Path) -> str:
    return path.read_text()


owner_lock = json.loads(read(OWNER / "rspyts.lock"))
consumer_lock = json.loads(read(CONSUMER / "rspyts.lock"))
owner_typescript = OWNER / ".rspyts" / "typescript"
consumer_typescript = CONSUMER / ".rspyts" / "typescript"

owner_package = json.loads(read(owner_typescript / "package.json"))
owner_wasm_types = read(owner_typescript / "index.d.ts")
owner_wire_types = read(owner_typescript / "wire.d.ts")
owner_wire_runtime = read(owner_typescript / "wire.js")
consumer_package = json.loads(read(consumer_typescript / "package.json"))
consumer_types = read(consumer_typescript / "index.d.ts")
consumer_runtime = read(consumer_typescript / "index.js")
consumer_models = read(
    CONSUMER
    / ".rspyts"
    / "python"
    / "example"
    / "consumer"
    / "contracts"
    / "models.py"
)
consumer_functions = read(
    CONSUMER
    / ".rspyts"
    / "python"
    / "example"
    / "consumer"
    / "contracts"
    / "functions.py"
)
consumer_init = read(
    CONSUMER
    / ".rspyts"
    / "python"
    / "example"
    / "consumer"
    / "contracts"
    / "__init__.py"
)
consumer_contract = json.loads(read(CONSUMER / ".rspyts" / "contract.json"))

owner_fingerprint = owner_lock["fingerprint"]
owner_version = owner_lock["manifest"]["crateVersion"]

assert owner_lock["hosts"]["typescript"]["mode"] == "wasm"
assert owner_package["exports"]["./wire"] == {
    "default": "./wire.js",
    "import": "./wire.js",
    "types": "./wire.d.ts",
}
assert "sideEffects" not in owner_package
assert "wire.js" in owner_package["files"]
assert "wire.d.ts" in owner_package["files"]
assert f'CONTRACT_FINGERPRINT: "{owner_fingerprint}"' in owner_wire_types
assert f'CONTRACT_FINGERPRINT = "{owner_fingerprint}"' in owner_wire_runtime

# The executable API retains bigint and typed-array semantics.
assert "interface NativeCounter" in owner_wasm_types
assert "readonly value: bigint;" in owner_wasm_types
assert "readonly tag: globalThis.Uint8Array;" in owner_wasm_types

# The canonical wire subpath contains only complete, static-safe shapes.
assert "NativeCounter" not in owner_wire_types
assert "interface Quantity" in owner_wire_types
assert "readonly numerator: number;" in owner_wire_types
assert "readonly denominator: number;" in owner_wire_types
assert "enum ItemKind" in owner_wire_types
assert "readonly kind?: ItemKind | null;" in owner_wire_types
assert "readonly tag: readonly number[] & { readonly length: 4 };" in owner_wire_types

# The consumer imports the owner's wire identity; it never copies it.
assert (
    'import type { Item, Quantity } from "@example/owner/wire";'
    in consumer_types
)
assert 'import { ItemKind } from "@example/owner/wire";' in consumer_types
assert "interface Item" not in consumer_types
assert "interface Quantity" not in consumer_types
assert "enum ItemKind" not in consumer_types
assert "NativeCounter" not in consumer_types
assert "counterIsNonzero" not in consumer_types
assert "select" not in consumer_types
assert consumer_package["peerDependencies"] == {"@example/owner": owner_version}
assert "sideEffects" not in consumer_package
assert 'from "@example/owner/wire";' in consumer_runtime
assert owner_fingerprint in consumer_runtime

# Python preserves the same class object and guards the installed owner.
assert "class Item" not in consumer_models
assert "from example.owner.contracts import" in consumer_models
assert "def counter_is_nonzero" in consumer_functions
assert (
    "from example.owner.contracts import CONTRACT_FINGERPRINT as "
    "__rspyts_owner_contract_fingerprint__"
    in consumer_init
)
assert "del __rspyts_owner_contract_fingerprint__" in consumer_init

assert all(
    function["owner"] == "cross-package-consumer"
    for function in consumer_contract["manifest"]["functions"]
)
dependency = consumer_lock["dependencies"]["owner"]
assert dependency["fingerprint"] == owner_fingerprint
assert dependency["crate"] == "cross-package-owner"
assert dependency["crateVersion"] == owner_version
assert dependency["python"] == "example.owner.contracts"
assert dependency["typescript"] == {
    "package": "@example/owner",
    "mode": "wasm",
}

def install_typescript_packages(directory: Path) -> Path:
    scope = directory / "node_modules" / "@example"
    scope.mkdir(parents=True)
    shutil.copytree(owner_typescript, scope / "owner")
    shutil.copytree(consumer_typescript, scope / "consumer")
    (directory / "package.json").write_text('{"type":"module"}\n')
    return scope


# A normal ESM import executes the dependency guard.
with tempfile.TemporaryDirectory() as temporary:
    directory = Path(temporary)
    scope = install_typescript_packages(directory)
    imported = subprocess.run(
        [
            "node",
            "--input-type=module",
            "-e",
            (
                'import { ItemKind } from "@example/consumer";'
                'if (ItemKind.Standard !== "standard") '
                'throw new Error("wrong enum");'
            ),
        ],
        cwd=directory,
        check=False,
        capture_output=True,
        text=True,
    )
    assert imported.returncode == 0, imported.stderr

    wire = scope / "owner" / "wire.js"
    source = read(wire)
    tampered = source.replace(owner_fingerprint, "sha256:" + "0" * 64)
    assert tampered != source
    wire.write_text(tampered)
    mismatch = subprocess.run(
        [
            "node",
            "--input-type=module",
            "-e",
            'import "@example/consumer";',
        ],
        cwd=directory,
        check=False,
        capture_output=True,
        text=True,
    )
    assert mismatch.returncode != 0
    assert "fingerprint mismatch" in mismatch.stderr

# Vite/Rollup must retain the guard while bundling a re-exported wire enum.
vite = (
    ROOT
    / "owner"
    / "typescript"
    / "node_modules"
    / ".bin"
    / "vite"
)
assert vite.exists(), f"bundler acceptance requires {vite}"
with tempfile.TemporaryDirectory() as temporary:
    directory = Path(temporary)
    scope = install_typescript_packages(directory)
    wire = scope / "owner" / "wire.js"
    source = read(wire)
    wire.write_text(source.replace(owner_fingerprint, "sha256:" + "f" * 64))
    (directory / "entry.mjs").write_text(
        'import { ItemKind } from "@example/consumer";\n'
        'if (ItemKind.Standard !== "standard") throw new Error("wrong enum");\n'
        "export default ItemKind.Standard;\n"
    )
    (directory / "vite.config.mjs").write_text(
        "export default {\n"
        '  build: { ssr: "entry.mjs", outDir: "dist", '
        'rollupOptions: { output: { entryFileNames: "bundle.mjs" } } },\n'
        "  ssr: { noExternal: true },\n"
        "};\n"
    )
    bundled = subprocess.run(
        [str(vite), "build", "--config", "vite.config.mjs"],
        cwd=directory,
        check=False,
        capture_output=True,
        text=True,
    )
    assert bundled.returncode == 0, bundled.stdout + bundled.stderr
    mismatch = subprocess.run(
        ["node", "dist/bundle.mjs"],
        cwd=directory,
        check=False,
        capture_output=True,
        text=True,
    )
    assert mismatch.returncode != 0
    assert "fingerprint mismatch" in mismatch.stderr
