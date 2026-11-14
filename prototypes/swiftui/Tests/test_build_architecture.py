"""Exercise build.sh's real lipo checks with disposable Mach-O files; no app launch."""
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest


@unittest.skipUnless(platform.system() == 'Darwin', 'Requires Apple clang and lipo')
class BuildArchitectureChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory(prefix='dlm-build-architecture-')
        cls.addClassCleanup(cls.scratch.cleanup)
        cls.root = Path(cls.scratch.name)
        cls.arch = platform.machine()
        cls.binary = cls.root / 'fixture'
        subprocess.run(['xcrun', 'clang', '-arch', cls.arch, '-x', 'c', '-', '-o', str(cls.binary)],
                       input='int main(void) { return 0; }\n', text=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
        build = Path(__file__).resolve().parents[1] / 'build.sh'
        lines = [line for line in build.read_text().splitlines()
                 if line.strip().startswith('lipo ') and '-verify_arch' in line]
        if not lines:
            raise AssertionError('Build script architecture checks are missing')
        cls.commands = '\n'.join(lines)

    def verify(self, *, helper_valid=True, app_valid=True, arch=None):
        with tempfile.TemporaryDirectory(dir=self.root) as directory:
            bundle = Path(directory) / 'DLM Native Development Debug.app'
            helper = bundle / 'Contents/Helpers/dlm-native-chrome-host'
            app = bundle / 'Contents/MacOS/DLMSwiftExperiment'
            for target, valid in [(helper, helper_valid), (app, app_valid)]:
                target.parent.mkdir(parents=True, exist_ok=True)
                if valid:
                    shutil.copyfile(self.binary, target)
                else:
                    target.write_bytes(b'not a Mach-O file')
            env = dict(os.environ, helper=str(helper), bundle=str(bundle), architecture=arch or self.arch)
            return subprocess.run(['/bin/sh', '-ec', self.commands], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE).returncode

    def test_matching_architecture_and_paths_with_spaces(self):
        self.assertEqual(self.verify(), 0)

    def test_wrong_architecture_is_rejected(self):
        other = 'x86_64' if self.arch == 'arm64' else 'arm64'
        self.assertNotEqual(self.verify(arch=other), 0)

    def test_invalid_helper_is_rejected(self):
        self.assertNotEqual(self.verify(helper_valid=False), 0)

    def test_invalid_application_is_rejected(self):
        self.assertNotEqual(self.verify(app_valid=False), 0)


if __name__ == '__main__':
    unittest.main()
