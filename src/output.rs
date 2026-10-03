use anyhow::{ensure, Context, Result};
use std::{
    fs::File,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub struct AtomicOutput {
    temp: NamedTempFile,
    destination: PathBuf,
    overwrite: bool,
}
impl AtomicOutput {
    pub fn new(destination: &Path, input: &str, overwrite: bool) -> Result<Self> {
        if input != "-" && destination.exists() {
            ensure!(
                !same_file::is_same_file(input, destination)?,
                "input and output refer to the same file"
            );
        }
        ensure!(
            overwrite || !destination.exists(),
            "output exists; use --overwrite"
        );
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temp = tempfile::Builder::new()
            .prefix(".canlog-")
            .suffix(".partial")
            .tempfile_in(parent)
            .context("creating temporary output")?;
        Ok(Self {
            temp,
            destination: destination.into(),
            overwrite,
        })
    }
    pub fn file(&self) -> Result<File> {
        Ok(self.temp.as_file().try_clone()?)
    }
    pub fn temporary_path(&self) -> &Path {
        self.temp.path()
    }
    pub fn publish(self, sync: bool) -> Result<()> {
        if sync {
            self.temp
                .as_file()
                .sync_all()
                .context("syncing finalized output")?;
        }
        if self.overwrite {
            self.temp
                .persist(&self.destination)
                .context("publishing output")?;
        } else {
            self.temp
                .persist_noclobber(&self.destination)
                .context("publishing without overwrite")?;
        }
        Ok(())
    }
}

pub fn write_report(path: &Path, value: &impl serde::Serialize, overwrite: bool) -> Result<()> {
    let output = AtomicOutput::new(path, "-", overwrite)?;
    let mut file = output.file()?;
    serde_json::to_writer_pretty(&mut file, value)?;
    use std::io::Write;
    writeln!(file)?;
    drop(file);
    output.publish(true)
}

pub fn ensure_distinct_paths(a: &Path, b: &Path) -> Result<()> {
    if a.exists() && b.exists() {
        ensure!(
            !same_file::is_same_file(a, b)?,
            "output and report paths conflict"
        );
    }
    fn normalized(path: &Path) -> Result<PathBuf> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        // A protected source's entire parent directory may have moved. Resolve
        // the nearest existing ancestor while retaining the missing suffix.
        let mut ancestor = parent;
        let canonical = loop {
            match ancestor.canonicalize() {
                Ok(path) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let next = ancestor
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new("."));
                    if next == ancestor {
                        return Err(e.into());
                    }
                    ancestor = next;
                }
                Err(e) => return Err(e.into()),
            }
        };
        let mut normalized = canonical;
        let suffix = if ancestor == Path::new(".") && !parent.is_absolute() {
            parent
        } else {
            parent.strip_prefix(ancestor)?
        };
        for component in suffix.components() {
            match component {
                std::path::Component::Normal(part) => normalized.push(part),
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::CurDir => {}
                _ => anyhow::bail!("invalid path suffix"),
            }
        }
        Ok(normalized.join(path.file_name().context("output filename required")?))
    }
    let a = normalized(a)?;
    let b = normalized(b)?;
    #[cfg(windows)]
    let equal = a
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy());
    #[cfg(not(windows))]
    let equal = a == b;
    ensure!(!equal, "output and report paths conflict");
    Ok(())
}
