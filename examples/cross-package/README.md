# Owner and consumer contracts

The owner combines authored Python source with generated contracts and a WASM
package. The consumer combines authored Python source with generated contracts
and a static TypeScript package that imports the owner's canonical `./wire`
types instead of generating copies. The fixture also checks exact peer
versions, runtime fingerprints, and stale-lock rejection.

Install the WASM target and matching wasm-bindgen CLI, then run from the
repository root:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version '=0.2.126' --locked
cargo build --locked -p rspyts-cli
rspyts="$PWD/target/debug/rspyts"
npm ci --prefix examples/cross-package/owner/typescript

python3.11 -m venv /tmp/rspyts-cross-package
python=/tmp/rspyts-cross-package/bin/python
"$python" -m pip install "maturin>=1.9,<2" "pydantic>=2.11" "numpy>=2"

"$rspyts" build --config examples/cross-package/owner/rspyts.toml
"$rspyts" check --locked --config examples/cross-package/owner/rspyts.toml
"$rspyts" build --config examples/cross-package/consumer/rspyts.toml
"$rspyts" check --locked --config examples/cross-package/consumer/rspyts.toml
"$python" examples/cross-package/assert_identity.py
RSPYTS_BIN="$rspyts" \
  "$python" examples/cross-package/assert_stale_locks.py

(cd examples/cross-package/owner/python && \
  "$python" -m maturin build --release --out /tmp/rspyts-owner-wheels)
(cd examples/cross-package/consumer/python && \
  "$python" -m maturin build --release --out /tmp/rspyts-consumer-wheels)
"$python" -m pip install \
  /tmp/rspyts-owner-wheels/*.whl \
  /tmp/rspyts-consumer-wheels/*.whl
"$python" examples/cross-package/assert_runtime_identity.py
```

The runtime identity check proves the installed packages share the owner's
exact Python class and reject a mismatched owner fingerprint.
