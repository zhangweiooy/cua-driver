"""Build the development .app and unsigned installer on an Apple Silicon Mac."""
import argparse
import os
from pathlib import Path
import plistlib
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import package


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
    workspaces = dict(driver=package.REPO / 'libs/cua-driver/rust',
                      perception=package.CACHE / lock['perception']['tag'] / 'source/libs/cua-driver/rust')
    outputs = {}
    environment = dict(os.environ, MACOSX_DEPLOYMENT_TARGET='13.0')
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
    info = dict(CFBundleExecutable='cua', CFBundleIdentifier=config['bundle_id'],
                CFBundleName=config['app_name'], CFBundleDisplayName=config['app_name'],
                CFBundlePackageType='APPL', CFBundleVersion=release_version,
                CFBundleShortVersionString=release_version, LSMinimumSystemVersion='13.0',
                LSUIElement=True, NSHighResolutionCapable=True)
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
