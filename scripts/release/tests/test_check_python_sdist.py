from __future__ import annotations

import io
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "check-python-sdist.py"


class CheckPythonSdistTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary_directory.cleanup)
        self.root = Path(self.temporary_directory.name)
        self.package = self.root / "package"
        (self.package / "src" / "demo").mkdir(parents=True)
        (self.package / "tests").mkdir()
        for path in ("LICENSE", "README.md", "pyproject.toml"):
            (self.package / path).write_text(path, encoding="utf-8")
        (self.package / "src" / "demo" / "__init__.py").write_text("", encoding="utf-8")
        (self.package / "tests" / "test_demo.py").write_text("", encoding="utf-8")

    def write_sdist(
        self,
        *,
        duplicate: str | None = None,
        extra: str | None = None,
        omit: str | None = None,
    ) -> Path:
        paths = [
            "LICENSE",
            "PKG-INFO",
            "README.md",
            "pyproject.toml",
            "src/demo/__init__.py",
            "tests/test_demo.py",
        ]
        if omit is not None:
            paths.remove(omit)
        if extra is not None:
            paths.append(extra)
        if duplicate is not None:
            paths.append(duplicate)

        sdist = self.root / "demo-1.0.0.tar.gz"
        with tarfile.open(sdist, "w:gz") as archive:
            for relative in paths:
                data = relative.encode()
                member = tarfile.TarInfo(f"demo-1.0.0/{relative}")
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
        return sdist

    def run_check(self, sdist: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), str(sdist), str(self.package)],
            check=False,
            capture_output=True,
            text=True,
        )

    def test_accepts_exact_source_set(self) -> None:
        self.assertEqual(self.run_check(self.write_sdist()).returncode, 0)

    def test_rejects_missing_source(self) -> None:
        result = self.run_check(self.write_sdist(omit="tests/test_demo.py"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing from sdist", result.stderr)

    def test_rejects_unexpected_source(self) -> None:
        result = self.run_check(self.write_sdist(extra="secret.txt"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("unexpected in sdist", result.stderr)

    def test_rejects_duplicate_members(self) -> None:
        result = self.run_check(self.write_sdist(duplicate="LICENSE"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("duplicate sdist member", result.stderr)

    def test_rejects_unsafe_members(self) -> None:
        sdist = self.write_sdist(extra="../outside.txt")
        result = self.run_check(sdist)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unsafe sdist member", result.stderr)

    def test_rejects_links(self) -> None:
        sdist = self.write_sdist()
        replacement = self.root / "linked.tar.gz"
        with (
            tarfile.open(sdist, "r:gz") as source,
            tarfile.open(replacement, "w:gz") as target,
        ):
            for member in source.getmembers():
                file = source.extractfile(member) if member.isfile() else None
                target.addfile(member, file)
            member = tarfile.TarInfo("demo-1.0.0/src/demo/linked.py")
            member.type = tarfile.SYMTYPE
            member.linkname = "__init__.py"
            target.addfile(member)
        result = self.run_check(replacement)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unsupported sdist member", result.stderr)


if __name__ == "__main__":
    unittest.main()
