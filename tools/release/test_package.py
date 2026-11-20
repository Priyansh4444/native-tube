import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

from package import FRONTENDS, TARGETS, app_title, checksum, inspect_binary, verify


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
        title = app_title(frontend)
        binary_rel = f"{title}.app/Contents/MacOS/nt-{frontend}"
        binary = b"test binary"
        files = {
            binary_rel: (binary, 0o755 if executable else 0o644),
            f"{title}.app/Contents/Info.plist": (b"<plist/>", 0o644),
            f"{title}.app/Contents/Frameworks/libmpv.2.dylib": (b"mpv", 0o755),
            f"{title}.app/Contents/Resources/RUN.txt": (b"test\n", 0o644),
            f"{title}.app/Contents/Resources/Cargo.lock": (b"test\n", 0o644),
            f"{title}.app/Contents/Resources/DEPENDENCIES.md": (b"test\n", 0o644),
            f"{title}.app/Contents/Resources/licenses/project/NOTICE": (b"test\n", 0o644),
            f"{title}.app/Contents/Resources/BUILD.json": (json.dumps({
                "frontend": frontend,
                "target": target,
                "binary_sha256": hashlib.sha256(binary).hexdigest(),
                "package_kind": "fresh-build",
                "source_revision": self.commit,
                "source_ref": self.ref,
                "github_run_id": "123",
            }).encode(), 0o644),
        }
        folder = self.artifacts / name
        folder.mkdir(exist_ok=True)
        archive = folder / f"{name}.zip"
        with zipfile.ZipFile(archive, "w") as zipped:
            for path, (payload, mode) in files.items():
                info = zipfile.ZipInfo(path)
                info.external_attr = mode << 16
                info.create_system = 3
                zipped.writestr(info, payload)
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
        self.write_archive("gpui", "macos-arm64", executable=False)
        with self.assertRaisesRegex(ValueError, "Missing executable permission"):
            verify(self.artifacts, self.commit, self.ref, "123")

    def test_tampered_archive(self):
        archive, _ = self.write_archive("iced", "macos-arm64")
        with archive.open("ab") as file:
            file.write(b"tampered")
        with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
            verify(self.artifacts, self.commit, self.ref, "123")

    def test_mismatched_source_revision(self):
        with self.assertRaisesRegex(ValueError, "Build provenance mismatch"):
            verify(self.artifacts, "0" * 40, self.ref, "123")

    def test_missing_binary_input(self):
        with self.assertRaisesRegex(ValueError, "Missing executable"):
            inspect_binary(self.root / "missing", "macos-arm64")


if __name__ == "__main__":
    unittest.main()
