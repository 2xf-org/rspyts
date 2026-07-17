from __future__ import annotations

import io
import tarfile
import tempfile
import unittest
from pathlib import Path

from scripts.release.compare_crate_archives import ComparisonError, compare_archives


CHECKSUM = "a" * 64


def lock(provenance: str = "", dependency: str = "") -> bytes:
    return f'''version = 4

[[package]]
name = "example"
version = "0.1.0"
dependencies = [
 "rspyts-macros",
]
{dependency}

[[package]]
name = "rspyts-macros"
version = "0.4.3"
{provenance}
'''.encode()


def archive(
    path: Path,
    cargo_lock: bytes,
    source: bytes = b"pub fn example() {}\n",
) -> None:
    with tarfile.open(path, mode="w:gz") as output:
        for name, contents in {
            "example-0.1.0/Cargo.lock": cargo_lock,
            "example-0.1.0/src/lib.rs": source,
        }.items():
            member = tarfile.TarInfo(name)
            member.size = len(contents)
            member.mtime = 1
            output.addfile(member, io.BytesIO(contents))


class CompareCrateArchivesTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.candidate = self.root / "candidate.crate"
        self.repacked = self.root / "repacked.crate"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_accepts_only_expected_registry_provenance(self) -> None:
        archive(self.candidate, lock())
        archive(
            self.repacked,
            lock(
                'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
                f'checksum = "{CHECKSUM}"'
            ),
        )

        compare_archives(
            self.candidate,
            self.repacked,
            {("rspyts-macros", "0.4.3"): CHECKSUM},
        )

    def test_accepts_verified_provenance_in_both_archives(self) -> None:
        provenance = (
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            f'checksum = "{CHECKSUM}"'
        )
        archive(self.candidate, lock(provenance))
        archive(self.repacked, lock(provenance))

        compare_archives(
            self.candidate,
            self.repacked,
            {("rspyts-macros", "0.4.3"): CHECKSUM},
        )

    def test_rejects_wrong_registry_checksum(self) -> None:
        archive(self.candidate, lock())
        archive(
            self.repacked,
            lock(
                'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
                f'checksum = "{"b" * 64}"'
            ),
        )

        with self.assertRaisesRegex(ComparisonError, "unexpected provenance"):
            compare_archives(
                self.candidate,
                self.repacked,
                {("rspyts-macros", "0.4.3"): CHECKSUM},
            )

    def test_rejects_unapproved_lock_change(self) -> None:
        archive(self.candidate, lock())
        archive(self.repacked, lock(dependency='checksum = "changed"'))

        with self.assertRaisesRegex(ComparisonError, "beyond approved"):
            compare_archives(
                self.candidate,
                self.repacked,
                {("rspyts-macros", "0.4.3"): CHECKSUM},
            )

    def test_rejects_source_change(self) -> None:
        archive(self.candidate, lock())
        archive(self.repacked, lock(), source=b"pub fn changed() {}\n")

        with self.assertRaisesRegex(ComparisonError, "outside Cargo.lock"):
            compare_archives(
                self.candidate,
                self.repacked,
                {("rspyts-macros", "0.4.3"): CHECKSUM},
            )


if __name__ == "__main__":
    unittest.main()
