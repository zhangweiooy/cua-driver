//! Shared fixed local product bundle. No official extension catalog or PATH lookup.
use anyhow::{bail, Context, Result};
use cua_driver_core::perception_client::{PerceptionClient, PerceptionWorkerConfig};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

const LOCK: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../../products/cua/upstream.lock.json"
));
const MANIFEST: &str = "cua-package.json";
static PERCEPTION: OnceLock<PerceptionClient> = OnceLock::new();

#[derive(Deserialize)]
struct Release {
    tag: String,
    commit: String,
}
#[derive(Deserialize)]
struct Lock {
    product_version: String,
    driver: Release,
    perception: Release,
    targets: BTreeMap<String, Target>,
}
#[derive(Deserialize)]
pub(crate) struct Target {
    product: String,
    pub cli_name: String,
    binary: String,
    directory_from_executable: String,
    state_env: String,
    state_path: String,
    state_override: String,
    perception_dir: String,
    worker: String,
    runtime_library: String,
    runtime_dependencies: Option<RuntimeFiles>,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
}
const TARGET: &str = env!("CUA_PRODUCT_TARGET");
fn lock() -> &'static Lock {
    static LOCK_DATA: OnceLock<Lock> = OnceLock::new();
    LOCK_DATA.get_or_init(|| serde_json::from_str(LOCK).expect("build-time validated product lock"))
}
pub(crate) fn target() -> &'static Target {
    lock()
        .targets
        .get(TARGET)
        .expect("product target must be pinned")
}
fn resource(name: &str) -> String {
    format!("{}/{name}", target().perception_dir)
}

#[derive(Deserialize)]
struct RuntimeFiles {
    files: Vec<RuntimeFile>,
}
#[derive(Deserialize)]
struct RuntimeFile {
    name: String,
    sha256: String,
}
#[derive(Deserialize)]
struct Bundle {
    schema_version: u32,
    product: String,
    product_version: String,
    target: String,
    driver: Release,
    perception: Release,
    protocol_version: u32,
    files: BTreeMap<String, String>,
}

fn root() -> Result<PathBuf> {
    let exe = fs::canonicalize(std::env::current_exe()?)?;
    Ok(exe
        .parent()
        .context("executable has no directory")?
        .join(&target().directory_from_executable)
        .canonicalize()?)
}

pub(crate) fn state_namespace() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let path = std::env::current_exe().expect("current executable path");
        let identity = path
            .parent()
            .expect("installation directory")
            .to_string_lossy()
            .to_lowercase();
        format!(
            "{}-{:x}",
            target().cli_name,
            Sha256::digest(identity.as_bytes())
        )
    })
}

fn owned_file(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains(':')
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        bail!("invalid package-relative path: {relative}");
    }
    let mut current = root.to_path_buf();
    for part in path.components() {
        current.push(part);
        let metadata = fs::symlink_metadata(&current).with_context(|| {
            format!("missing package file: {relative}; repair the product installation")
        })?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                bail!("package path contains a reparse point: {relative}");
            }
        }
        if metadata.file_type().is_symlink() {
            bail!("package path contains a symlink: {relative}");
        }
    }
    if !current.is_file() {
        bail!("package resource is not a file: {relative}");
    }
    Ok(current)
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn verify_bundle(root: &Path) -> Result<Bundle> {
    let bundle = read_bundle(root)?;
    for (relative, expected) in &bundle.files {
        if sha256(&root.join(relative))? != *expected {
            bail!("package SHA256 mismatch: {relative}; repair the product installation");
        }
    }
    Ok(bundle)
}

// Once per process: check package identity, resource paths and runtime contract.
// Payload hashing belongs to package-check and upstream model loading.
fn read_bundle(root: &Path) -> Result<Bundle> {
    let manifest = owned_file(root, MANIFEST)?;
    let bundle: Bundle =
        serde_json::from_slice(&fs::read(manifest)?).context("invalid product package manifest")?;
    let lock = lock();
    if bundle.schema_version != 1
        || bundle.product != target().product
        || bundle.product_version != lock.product_version
        || bundle.target != TARGET
        || bundle.protocol_version != 1
        || bundle.driver.tag != lock.driver.tag
        || bundle.driver.commit != lock.driver.commit
        || bundle.perception.tag != lock.perception.tag
        || bundle.perception.commit != lock.perception.commit
    {
        bail!("package identity/protocol mismatch; repair the product installation");
    }
    for required in [
        target().binary.clone(),
        resource(&target().worker),
        resource("model-manifest.json"),
        resource(&target().runtime_library),
        resource("runtime-contract.json"),
    ] {
        if !bundle.files.contains_key(&required) {
            bail!("package manifest omits {required}");
        }
    }
    if let Some(runtime) = &target().runtime_dependencies {
        for item in &runtime.files {
            let relative = resource(&item.name);
            if bundle.files.get(&relative) != Some(&item.sha256) {
                bail!("package omits or mismatches required CRT resource: {relative}");
            }
        }
    }
    for (relative, expected) in &bundle.files {
        owned_file(root, relative)?;
        if expected.len() != 64 {
            bail!("invalid package SHA256: {relative}");
        }
    }
    let contract: serde_json::Value = serde_json::from_slice(&fs::read(owned_file(
        root,
        &resource("runtime-contract.json"),
    )?)?)?;
    if contract["schemaVersion"] != 1
        || contract["protocolVersion"] != bundle.protocol_version
        || contract["target"] != bundle.target
        || contract["rejectMismatch"] != true
    {
        bail!("perception runtime contract mismatch");
    }
    let mut resources = vec![
        (format!("{}/", target().perception_dir), &contract["worker"]),
        (
            format!("{}/", target().perception_dir),
            &contract["modelManifest"],
        ),
        (
            format!("{}/runtime/", target().perception_dir),
            &contract["runtime"],
        ),
        (
            format!("{}/models/", target().perception_dir),
            &contract["dictionary"],
        ),
    ];
    let models = contract["models"]
        .as_array()
        .context("runtime contract has no models")?;
    if models.is_empty() {
        bail!("runtime contract has no models");
    }
    resources.extend(
        models
            .iter()
            .map(|model| (format!("{}/models/", target().perception_dir), model)),
    );
    for (prefix, resource) in resources {
        let name = resource["name"]
            .as_str()
            .context("invalid runtime resource name")?;
        let expected = resource["sha256"]
            .as_str()
            .context("invalid runtime resource digest")?;
        let relative = format!("{prefix}{name}");
        if bundle.files.get(&relative).map(String::as_str) != Some(expected) {
            bail!("package does not bind runtime resource: {relative}");
        }
    }
    Ok(bundle)
}

pub(crate) fn initialize() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lock = lock();
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        println!(
            "{} {} (Cua Driver {}; product build)",
            target().cli_name,
            lock.product_version,
            env!("CARGO_PKG_VERSION")
        );
        return Ok(true);
    }
    let state = std::env::var_os(&target().state_override)
        .map(PathBuf::from)
        .unwrap_or(
            PathBuf::from(
                std::env::var_os(&target().state_env)
                    .with_context(|| format!("{} not set", target().state_env))?,
            )
            .join(&target().state_path),
        );
    if !state.is_absolute() {
        bail!("{} must be absolute", target().state_override);
    }
    std::env::set_var("CUA_DRIVER_RS_HOME", &state);
    std::env::set_var("CUA_DRIVER_CONFIG_PATH", state.join("config.json"));
    std::env::set_var("CUA_DRIVER_RS_UPDATE_CHECK", "false");
    std::env::set_var("DO_NOT_TRACK", "1");
    // A broken bundle must still be stoppable. Help/schema reads need no models.
    if matches!(
        args.first().map(String::as_str),
        Some("stop" | "status" | "--help" | "-h" | "list-tools" | "describe" | "mcp-config")
    ) {
        return Ok(false);
    }
    let root = root()?;
    if args.first().map(String::as_str) == Some("package-check") {
        let bundle = verify_bundle(&root)?;
        println!(
            "{}",
            serde_json::json!({"success":true,"product":target().product,"version":bundle.product_version,"files_verified":bundle.files.len(),"ocr":"PP-OCRv5 Chinese candidate; full quality acceptance pending","windows_1903_verified":false})
        );
        return Ok(true);
    }
    let bundle = read_bundle(&root)?;
    let _ = PERCEPTION.set(client_at(&root, &bundle)?);
    Ok(false)
}

pub(crate) fn validate_command(command: &crate::cli::Command) -> Result<()> {
    use crate::cli::Command;
    if matches!(
        command,
        Command::Update { .. }
            | Command::CheckUpdate { .. }
            | Command::Channel { .. }
            | Command::Extension { .. }
            | Command::Autostart { .. }
            | Command::Skills { .. }
    ) {
        bail!("product components are managed together by the product installer; upstream component installation, updates and autostart are disabled");
    }
    Ok(())
}

fn client_at(root: &Path, bundle: &Bundle) -> Result<PerceptionClient> {
    let mut config = PerceptionWorkerConfig::installed_with_identity(
        root.join(resource(&target().worker)),
        "cua-perception",
        &bundle.product_version,
    );
    config.args = vec![
        "--manifest".into(),
        root.join(resource("model-manifest.json"))
            .to_str()
            .context("non-Unicode package path")?
            .into(),
        "--onnx-runtime-library".into(),
        root.join(resource(&target().runtime_library))
            .to_str()
            .context("non-Unicode runtime path")?
            .into(),
        "--extension-id".into(),
        "cua-perception".into(),
        "--extension-version".into(),
        bundle.product_version.clone(),
    ];
    PerceptionClient::new(config.with_bounded_reuse(Default::default()))
        .map_err(|e| anyhow::anyhow!("{}", e.message))
}

pub(crate) fn perception_client() -> PerceptionClient {
    PERCEPTION
        .get()
        .cloned()
        .unwrap_or_else(PerceptionClient::unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_paths_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        for path in ["../secret", "/absolute", "C:/secret", "nested\\secret", ""] {
            assert!(owned_file(dir.path(), path)
                .unwrap_err()
                .to_string()
                .contains("invalid package-relative path"));
        }
        fs::write(dir.path().join("resource"), "model").unwrap();
        assert!(owned_file(dir.path(), "resource").is_ok());
    }
    #[test]
    fn component_management_is_disabled() {
        assert!(validate_command(&crate::cli::Command::Update {
            apply: true,
            json: false
        })
        .unwrap_err()
        .to_string()
        .contains("managed together"));
        assert!(validate_command(&crate::cli::Command::Extension {
            args: vec!["remove".into(), "perception".into()]
        })
        .is_err());
        assert!(validate_command(&crate::cli::Command::Status { socket: None }).is_ok());
    }
    #[test]
    fn incomplete_or_mixed_package_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let lock: serde_json::Value = serde_json::from_str(LOCK).unwrap();
        let mut manifest = serde_json::json!({"schema_version":1,"product":target().product,"product_version":lock["product_version"],"target":TARGET,"protocol_version":1,"driver":lock["driver"],"perception":lock["perception"],"files":{}});
        fs::write(dir.path().join(MANIFEST), manifest.to_string()).unwrap();
        assert!(verify_bundle(dir.path())
            .err()
            .unwrap()
            .to_string()
            .contains(&format!("omits {}", target().binary)));
        manifest["protocol_version"] = 2.into();
        fs::write(dir.path().join(MANIFEST), manifest.to_string()).unwrap();
        assert!(verify_bundle(dir.path())
            .err()
            .unwrap()
            .to_string()
            .contains("identity/protocol mismatch"));
    }
}
