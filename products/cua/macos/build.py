"""Build the development .app and unsigned installer on an Apple Silicon Mac."""
import argparse
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tarfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import package


def app_template(lock, target):
    """Retain upstream bundle metadata/resources, never its signing identity."""
    archive = package.component_dir(lock, 'driver', target) / lock['targets'][target]['driver']['asset']
    with tarfile.open(archive) as tar:
        infos = [m for m in tar.getmembers() if m.isfile() and m.name.endswith('/CuaDriver.app/Contents/Info.plist')]
        if len(infos) != 1:
            raise ValueError('pinned Driver archive must contain one application bundle')
        info = plistlib.load(tar.extractfile(infos[0]))
        prefix = infos[0].name.removesuffix('Info.plist')
        resources = {}
        for member in tar.getmembers():
            if member.isfile() and member.name.startswith(prefix + 'Resources/'):
                relative = member.name.removeprefix(prefix)
                if '..' in Path(relative).parts:
                    raise ValueError('unsafe app resource path')
                resources[relative] = tar.extractfile(member).read()
    return info, resources


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('native macOS SDK and pkgbuild are required; run on a Mac')
    target = 'aarch64-apple-darwin'
    lock = package.read_json(package.ROOT / 'upstream.lock.json')
    config = lock['targets'][target]
    package.prepare(lock, target, args.offline)
    info, resources = app_template(lock, target)
    workspaces = dict(driver=package.REPO / 'libs/cua-driver/rust',
                      perception=package.CACHE / lock['perception']['tag'] / 'source/libs/cua-driver/rust')
    outputs = {}
    environment = dict(os.environ, MACOSX_DEPLOYMENT_TARGET=info['LSMinimumSystemVersion'])
    for component, workspace in workspaces.items():
        package.verify(workspace / 'Cargo.lock', lock[component]['cargo_lock_sha256'])
        output = package.component_dir(lock, component, target) / 'build'
        command = ['cargo', 'build', '--locked', '--release', '--target', target, '--target-dir', str(output),
                   '-p', 'cua-' + component, '--bin', 'cua-' + component]
        if component == 'driver':
            command += ['--features', 'cua-product']
        subprocess.run(command, cwd=workspace, env=environment, check=True)
        outputs[component] = output / target / 'release' / ('cua-' + component)
    staging = package.ROOT / 'out' / f'cua-{lock["product_version"]}-{target}'
    app = staging / 'Applications' / (config['app_name'] + '.app')
    contents = app / 'Contents'
    package.assemble(lock, target, outputs['driver'], outputs['perception'], contents)
    release_version = lock['product_version'].split('-')[0]
    info.update(CFBundleExecutable=config['cli_name'], CFBundleIdentifier=config['bundle_id'],
                CFBundleName=config['app_name'], CFBundleDisplayName=config['app_name'],
                CFBundlePackageType='APPL', CFBundleVersion=release_version,
                CFBundleShortVersionString=release_version,
                LSUIElement=True, NSHighResolutionCapable=True)
    for relative, data in resources.items():
        destination = contents / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
    with (contents / 'Info.plist').open('wb') as stream:
        plistlib.dump(info, stream)
    package.seal(lock, target, contents)
    subprocess.run([str(contents / config['binary']), 'package-check'], check=True)
    installer = package.ROOT / 'out' / f'cua-{lock["product_version"]}-macos-arm64.pkg'
    if installer.exists():
        raise FileExistsError(f'preserve existing installer: {installer}')
    subprocess.run(['pkgbuild', '--root', str(staging), '--identifier', config['bundle_id'],
                    '--version', lock['product_version'].split('-')[0], '--install-location', '/', str(installer)], check=True)
    package.sources(lock)
    print(installer)


if __name__ == '__main__':
    main()
