//! File-backed artifact store under `<workdir>/artifacts/` used by the
//! Railgun SDK for snark proving artifacts and the wallet manifest.

use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug)]
pub struct Artifact {
    workdir: PathBuf,
    root: PathBuf,
}

impl Artifact {
    #[must_use]
    pub fn new(workdir: &Path) -> Self {
        Self {
            workdir: workdir.to_path_buf(),
            root: workdir.join("artifacts"),
        }
    }

    #[must_use]
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Read a Railgun artifact as raw bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the artifact exists but cannot be read.
    pub fn read(&self, relative_path: &str) -> Result<Vec<u8>> {
        std::fs::read(self.root.join(relative_path)).map_err(Into::into)
    }

    /// Write a Railgun artifact under the artifact root.
    ///
    /// `dir` and `relative_path` come from the SDK's `ArtifactStore` callback
    /// signature `(dir, path, item)` and are not derivable from one another.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination directory cannot be created or the
    /// artifact cannot be written.
    pub fn write(&self, dir: &str, relative_path: &str, bytes: &[u8]) -> Result<()> {
        std::fs::create_dir_all(self.root.join(dir))?;
        std::fs::write(self.root.join(relative_path), bytes)?;
        Ok(())
    }

    #[must_use]
    pub fn exists(&self, relative_path: &str) -> bool {
        self.root.join(relative_path).exists()
    }
}
