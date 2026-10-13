"""Hermetic shared assembly rules; native executable behavior is tested separately."""
import copy
import hashlib
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import package


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.lock = package.read_json(package.ROOT / 'upstream.lock.json')

    def test_model_overlay_uses_one_definition_and_preserves_detector(self):
        original = {'identity': {'source_revision': 'pinned-upstream'},
                    'detector': {'model': {'path': 'models/icon.onnx', 'sha256': 'a' * 64}},
                    'ocr': {'detector': {'model': {'path': 'models/detect.onnx', 'sha256': 'b' * 64}},
                            'recognizer': {'model': {}}, 'dictionary': {}}}
        before = copy.deepcopy(original)
        result = package.model_manifest(self.lock, original)
        self.assertEqual(original, before)
        self.assertEqual(result['detector'], before['detector'])
        self.assertEqual(result['ocr']['detector'], before['ocr']['detector'])
        self.assertEqual(result['ocr']['recognizer']['model']['sha256'], self.lock['chinese_ocr']['model']['sha256'])
        self.assertEqual(result['ocr']['dictionary']['path'], 'models/' + self.lock['chinese_ocr']['dictionary']['filename'])

    def test_target_layout_and_component_cache_are_separate(self):
        windows = self.lock['targets']['x86_64-pc-windows-msvc']
        mac = self.lock['targets']['aarch64-apple-darwin']
        self.assertEqual(windows['binary'], 'cua-win.exe')
        self.assertEqual(windows['state_path'], 'cua-win/rust')
        self.assertEqual(mac['binary'], 'MacOS/cua')
        self.assertEqual(mac['runtime_library'], 'runtime/libonnxruntime.dylib')
        paths = {package.component_dir(self.lock, component, target)
                 for component in ('driver', 'perception') for target in self.lock['targets']}
        self.assertEqual(len(paths), 4)

    def test_final_manifest_binds_each_targets_actual_files(self):
        for target, config in self.lock['targets'].items():
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                binary = root / config['binary']
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_bytes(b'fixture native binary')
                model = root / config['perception_dir'] / 'models/fixture.onnx'
                model.parent.mkdir(parents=True)
                model.write_bytes(b'model')
                package.seal(self.lock, target, root)
                manifest = package.read_json(root / package.MANIFEST)
                self.assertEqual(manifest['target'], target)
                self.assertEqual(manifest['product'], config['product'])
                self.assertEqual(set(manifest['files']), {config['binary'], model.relative_to(root).as_posix()})
                for relative, expected in manifest['files'].items():
                    package.verify(package.owned(root, relative), expected)
                model.write_bytes(b'damaged')
                with self.assertRaisesRegex(ValueError, 'SHA256 mismatch'):
                    package.verify(model, manifest['files'][model.relative_to(root).as_posix()])

    def test_unsafe_resource_and_missing_resource_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            for relative in ('../escape', '/absolute', 'C:/escape', 'nested\\escape', ''):
                with self.assertRaisesRegex(ValueError, 'invalid package-relative path'):
                    package.owned(temporary, relative)
            with self.assertRaisesRegex(ValueError, 'missing package file'):
                package.owned(temporary, 'absent')

    def test_offline_never_downloads_and_corruption_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'input'
            with self.assertRaisesRegex(FileNotFoundError, 'offline mode'):
                package.download(path, '0' * 64, 'https://invalid.example/', True)
            path.write_bytes(b'good')
            expected = hashlib.sha256(b'good').hexdigest()
            self.assertEqual(package.download(path, expected, 'https://invalid.example/', True), path)
            path.write_bytes(b'bad')
            with self.assertRaisesRegex(ValueError, 'SHA256 mismatch'):
                package.download(path, expected, 'https://invalid.example/', True)

    def test_source_export_never_overlays_a_previous_snapshot(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            previous = root / 'out/source' / self.lock['product_version']
            previous.mkdir(parents=True)
            marker = previous / 'old-source.rs'
            marker.write_text('preserve previous source', encoding='utf-8')
            with patch.object(package, 'ROOT', root):
                with self.assertRaisesRegex(FileExistsError, 'preserve existing source export'):
                    package.sources(self.lock)
            self.assertEqual(marker.read_text(encoding='utf-8'), 'preserve previous source')


if __name__ == '__main__':
    unittest.main()
