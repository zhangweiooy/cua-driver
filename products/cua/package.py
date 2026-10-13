"""Shared, standard-library-only build resource assembly. Never shipped as runtime."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent
CACHE = ROOT / '.cache'
MANIFEST = 'cua-package.json'


def read_json(path):
    return json.loads(Path(path).read_text(encoding='utf-8-sig'))


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verify(path, expected):
    if digest(path) != expected:
        raise ValueError(f'SHA256 mismatch: {path}')
    return Path(path)


def owned(root, relative):
    if not relative or '\\' in relative or ':' in relative or any(p in ('..', '.') for p in relative.split('/')) or relative.startswith('/'):
        raise ValueError(f'invalid package-relative path: {relative}')
    path = Path(root)
    for part in relative.split('/'):
        path /= part
        if path.is_symlink() or (hasattr(path, 'is_junction') and path.is_junction()):
            raise ValueError(f'linked package path: {relative}')
    if not path.is_file():
        raise ValueError(f'missing package file: {relative}')
    return path


def download(path, expected, url, offline):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        if offline:
            raise FileNotFoundError(f'locked input absent in offline mode: {path}')
        temporary = path.with_name(path.name + '.partial')
        with urllib.request.urlopen(url, timeout=120) as response, temporary.open('wb') as stream:
            shutil.copyfileobj(response, stream)
        verify(temporary, expected)
        temporary.replace(path)
    return verify(path, expected)


def component_dir(lock, component, target):
    return CACHE / lock[component]['tag'] / target


def prepare(lock, target, offline=False):
    config = lock['targets'][target]
    for component in ('driver', 'perception'):
        release = lock[component]
        actual = subprocess.check_output(['git', '-C', str(REPO), 'rev-parse', release['tag'] + '^{commit}'], text=True).strip()
        if actual != release['commit']:
            raise ValueError(f'release tag identity mismatch: {component}')
        archive = CACHE / release['tag'] / 'source.tar'
        archive.parent.mkdir(parents=True, exist_ok=True)
        if not archive.exists():
            subprocess.run(['git', '-C', str(REPO), 'archive', '--format=tar', f'--output={archive}', release['tag']], check=True)
        verify(archive, release['source_sha256'])
        source = archive.parent / 'source'
        source.mkdir(exist_ok=True)
        with tarfile.open(archive) as tar:
            tar.extractall(source, filter='data')
        asset = config[component]
        download(component_dir(lock, component, target) / asset['asset'], asset['sha256'],
                 f'https://github.com/trycua/cua/releases/download/{release["tag"]}/{asset["asset"]}', offline)
    perception = config['perception']
    archive = component_dir(lock, 'perception', target) / perception['asset']
    payload = archive.parent / 'payload'
    payload.mkdir(exist_ok=True)
    with tarfile.open(archive) as tar:
        tar.extractall(payload, filter='data')
    verify(payload / 'metadata/runtime-contract.json', perception['runtime_contract_sha256'])
    contract = read_json(payload / 'metadata/runtime-contract.json')
    if contract['target'] != target or contract['protocolVersion'] != lock['perception']['protocol_version']:
        raise ValueError('runtime contract target/protocol mismatch')
    for item in (lock['chinese_ocr']['model'], lock['chinese_ocr']['dictionary']):
        download(CACHE / 'models' / lock['chinese_ocr']['revision'] / item['filename'], item['sha256'], item['url'], offline)
    if 'runtime_dependencies' in config:
        runtime = config['runtime_dependencies']
        download(CACHE / 'crt' / runtime['version'] / target / runtime['filename'], runtime['sha256'], runtime['url'], offline)


def model_manifest(lock, original):
    result = copy.deepcopy(original)
    chinese = lock['chinese_ocr']
    result['identity'].update(name='omniparser-v2-ppocrv5-zh-cua', version=lock['product_version'],
                              source_revision=original['identity']['source_revision'] + ';chinese-ocr:' + chinese['revision'])
    for resource, definition in ((result['ocr']['recognizer']['model'], chinese['model']),
                                 (result['ocr']['dictionary'], chinese['dictionary'])):
        resource.update(path='models/' + definition['filename'], sha256=definition['sha256'])
    return result


def assemble(lock, target, driver, worker, output):
    config = lock['targets'][target]
    if output.exists():
        raise FileExistsError(f'preserve existing artifact and choose a new output: {output}')
    payload = component_dir(lock, 'perception', target) / 'payload'
    original_contract = payload / 'metadata/runtime-contract.json'
    verify(original_contract, config['perception']['runtime_contract_sha256'])
    contract = read_json(original_contract)
    if contract['target'] != target or contract['runtime']['name'] != Path(config['runtime_library']).name:
        raise ValueError('runtime layout does not match pinned target contract')
    original_manifest = owned(payload, contract['modelManifest']['name'])
    verify(original_manifest, contract['modelManifest']['sha256'])
    manifest = model_manifest(lock, read_json(original_manifest))
    perception = output / config['perception_dir']
    perception.mkdir(parents=True)
    binary = output / config['binary']
    binary.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(driver, binary)
    shutil.copy2(worker, perception / config['worker'])
    for resource in (manifest['detector']['model'], manifest['ocr']['detector']['model']):
        source = verify(owned(payload, resource['path']), resource['sha256'])
        destination = perception / resource['path']
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    for definition in (lock['chinese_ocr']['model'], lock['chinese_ocr']['dictionary']):
        source = CACHE / 'models' / lock['chinese_ocr']['revision'] / definition['filename']
        shutil.copy2(verify(source, definition['sha256']), perception / 'models' / definition['filename'])
    runtime = perception / config['runtime_library']
    runtime.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(verify(owned(payload, config['runtime_library']), contract['runtime']['sha256']), runtime)
    if 'runtime_dependencies' in config:
        crt = config['runtime_dependencies']
        with zipfile.ZipFile(CACHE / 'crt' / crt['version'] / target / crt['filename']) as archive:
            for item in crt['files']:
                destination = perception / item['name']
                destination.write_bytes(archive.read(crt['member_prefix'] + item['name']))
                verify(destination, item['sha256'])
    shutil.copytree(payload / 'notices', perception / 'notices')
    write_json(perception / 'model-manifest.json', manifest)
    contract['worker'].update(name=config['worker'], sha256=digest(perception / config['worker']))
    contract['modelManifest']['sha256'] = digest(perception / 'model-manifest.json')
    for model in contract['models']:
        if model['role'] == 'ocr-recognize':
            model.update(name=lock['chinese_ocr']['model']['filename'], sha256=lock['chinese_ocr']['model']['sha256'])
    contract['dictionary'].update(name=lock['chinese_ocr']['dictionary']['filename'], sha256=lock['chinese_ocr']['dictionary']['sha256'])
    write_json(perception / 'runtime-contract.json', contract)
    for name in ('upstream.lock.json', 'SOURCE_OFFER.md', 'MODEL_SOURCES.md', 'TEACHING.md'):
        shutil.copy2(ROOT / name, output / name)
    shutil.copy2(REPO / 'LICENSE.md', output / 'CUA-LICENSE.md')
    seal(lock, target, output)


def seal(lock, target, output):
    """Called after platform signing/bundle metadata so hashes bind final bytes."""
    files = {p.relative_to(output).as_posix(): digest(p) for p in sorted(output.rglob('*')) if p.is_file() and p != output / MANIFEST}
    write_json(output / MANIFEST, dict(schema_version=1, product=lock['targets'][target]['product'],
               product_version=lock['product_version'], target=target, driver=lock['driver'], perception=lock['perception'],
               protocol_version=lock['perception']['protocol_version'], files=files))


def sources(lock):
    output = ROOT / 'out/source' / lock['product_version']
    output.mkdir(parents=True, exist_ok=True)
    for component in ('driver', 'perception'):
        shutil.copy2(CACHE / lock[component]['tag'] / 'source.tar', output / (lock[component]['tag'] + '-source.tar'))
    with (output / 'downstream.patch').open('wb') as stream:
        subprocess.run(['git', '-C', str(REPO), 'diff', '--binary', lock['driver']['commit']], stdout=stream, check=True)
    shutil.copytree(ROOT, output / 'products/cua', ignore=shutil.ignore_patterns('.cache', 'out', '__pycache__'), dirs_exist_ok=True)
    for relative in subprocess.check_output(['git', '-C', str(REPO), 'ls-files', '--others', '--exclude-standard', 'libs/cua-driver/rust/crates'], text=True).splitlines():
        destination = output / 'new-files' / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(REPO / relative, destination)
    for target in lock['targets']:
        archive = component_dir(lock, 'perception', target) / lock['targets'][target]['perception']['asset']
        if archive.exists():
            shutil.copy2(archive, output / archive.name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('prepare', 'assemble', 'seal', 'sources'))
    parser.add_argument('--target', required=True)
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--driver', type=Path)
    parser.add_argument('--worker', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    lock = read_json(ROOT / 'upstream.lock.json')
    if args.target not in lock['targets']:
        parser.error('target is not pinned in upstream.lock.json')
    if args.action == 'prepare':
        prepare(lock, args.target, args.offline)
    elif args.action == 'sources':
        sources(lock)
    elif args.action == 'seal':
        seal(lock, args.target, args.output.resolve())
    else:
        assemble(lock, args.target, args.driver.resolve(strict=True), args.worker.resolve(strict=True), args.output.resolve())


if __name__ == '__main__':
    main()
