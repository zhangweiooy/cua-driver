use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
/// Fresh, session-owned output. Oryx reads and retains these files directly.
pub struct SessionOutput {
    pub root: PathBuf,
    files: Mutex<std::collections::BTreeSet<String>>,
}
impl SessionOutput {
    pub fn create(parent: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(parent)?;
        let root = fs::canonicalize(parent)?.join(format!("teaching-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root)?;
        for name in ["actions", "images", "evidence"] {
            fs::create_dir(root.join(name))?;
        }
        Ok(Self {
            root,
            files: Mutex::new(std::collections::BTreeSet::new()),
        })
    }
    pub fn write_new(&self, relative: &str, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join(relative))?;
        self.files.lock().unwrap().insert(relative.into());
        file.write_all(bytes)
    }
    pub fn write_json(&self, relative: &str, data: &Value) -> std::io::Result<()> {
        self.write_new(relative, &serde_json::to_vec_pretty(data)?)
    }
    pub fn state(&self, data: &Value) -> std::io::Result<()> {
        let temp = self.root.join("session.json.tmp");
        self.files
            .lock()
            .unwrap()
            .extend(["session.json.tmp".into(), "session.json".into()]);
        fs::write(&temp, serde_json::to_vec_pretty(data)?)?;
        fs::rename(temp, self.root.join("session.json"))
    }
    pub fn cancel(&self) -> std::io::Result<()> {
        // Only our generated root and recorded file names are deletion targets.
        fn check(path: &Path) -> std::io::Result<()> {
            let metadata = fs::symlink_metadata(path)?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(std::io::Error::other(
                        "output path replaced by reparse point",
                    ));
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::other("output path replaced by link"));
            }
            Ok(())
        }
        if fs::canonicalize(&self.root)? != self.root {
            return Err(std::io::Error::other("output directory identity changed"));
        }
        check(&self.root)?;
        for name in ["actions", "images", "evidence"] {
            check(&self.root.join(name))?;
        }
        for name in self.files.lock().unwrap().iter() {
            match fs::remove_file(self.root.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        // Unknown files belong to the caller, so a nonempty directory remains.
        for name in ["actions", "images", "evidence"] {
            let _ = fs::remove_dir(self.root.join(name));
        }
        let _ = fs::remove_dir(&self.root);
        Ok(())
    }
}

use super::{evidence, Timeline};
use serde_json::json;
pub(super) fn persist_final(t: &mut Timeline, evidence: &evidence::EvidenceLog) {
    let Some(output) = t.output.clone() else {
        return;
    };
    let mut data = t.snapshot(0, 0);
    data["evidence"] = evidence.snapshot(0, 0);
    if output.state(&data).is_err() {
        t.output_errors += 1;
        return;
    }
    if data["complete"] == true
        && output
            .write_json(
                "complete.json",
                &json!({
                    "cutoff_sequence": data["cutoff_sequence"],
                    "evidence_complete": data["evidence"]["complete"],
                    "evidence_cutoff_sequence": data["evidence"]["cutoff_sequence"]
                }),
            )
            .is_err()
    {
        t.output_errors += 1;
        data["complete"] = json!(false);
        data["diagnostics"]["output_errors"] = json!(t.output_errors);
        let _ = output.state(&data);
    }
}
