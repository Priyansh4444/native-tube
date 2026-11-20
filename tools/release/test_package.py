import hashlib
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from package import FRONTENDS, TARGETS, checksum, inspect_binary, verify


class PackageVerificationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.artifacts = self.root / "artifacts"
        self.artifacts.mkdir()
        self.commit = "9" * 40
        self.ref = "refs/tags/v0.1.0-rc.1"
        for frontend in FRONTENDS:
            for target in TARGETS:
                self.write_archive(frontend, target)

    def write_archive(self, frontend, target, executable=True):
        name = f"native-tube-{frontend}-{target}"
        stage = self.root / name
        stage.mkdir(exist_ok=True)
        binary = stage / f"nt-{frontend}"
        binary.write_bytes(b"test binary")
        binary.chmod(0o755 if executable else 0o644)
        for required in ("RUN.txt", "Cargo.lock", "DEPENDENCIES.md"):
            (stage / required).write_text("test\n")
        (stage / "licenses").mkdir(exist_ok=True)
        (stage / "BUILD.json").write_text(json.dumps({
            "frontend": frontend,
            "target": target,
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "package_kind": "fresh-build",
            "source_revision": self.commit,
            "source_ref": self.ref,
            "github_run_id": "123",
        }))
        folder = self.artifacts / name
        folder.mkdir(exist_ok=True)
        archive = folder / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(stage, arcname=name)
        digest = folder / f"{archive.name}.sha256"
        digest.write_text(checksum(archive))
        return archive, digest

    def test_complete_asset_set(self):
        verify(self.artifacts, self.commit, self.ref, "123")

    def test_missing_archive(self):
        archive, _ = self.write_archive("rune", "macos-arm64")
        archive.unlink()
        with self.assertRaisesRegex(ValueError, "Expected archive and checksum"):
            verify(self.artifacts, self.commit, self.ref, "123")

    def test_archive_without_execute_permission(self):
        self.write_archive("gpui", "linux-x86_64", executable=False)
        with self.assertRaisesRegex(ValueError, "Missing executable permission"):
            verify(self.artifacts, self.commit, self.ref, "123")

    def test_tampered_archive(self):
        archive, _ = self.write_archive("iced", "linux-x86_64")
        with archive.open("ab") as file:
            file.write(b"tampered")
        with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
            verify(self.artifacts, self.commit, self.ref, "123")

    def test_mismatched_source_revision(self):
        with self.assertRaisesRegex(ValueError, "Build provenance mismatch"):
            verify(self.artifacts, "0" * 40, self.ref, "123")

    def test_missing_binary_input(self):
        with self.assertRaisesRegex(ValueError, "Missing executable"):
            inspect_binary(self.root / "missing", "linux-x86_64")


if __name__ == "__main__":
    unittest.main()
