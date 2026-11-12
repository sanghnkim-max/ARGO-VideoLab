//! FilmCraft's host seam ([`filmcraft_engine::Services`]) confined to the workspace folder.
//!
//! The engine reads media, writes projects and exports through `Services`. VideoLab's
//! implementation forwards to FilmCraft's own [`FsServices`] after checking that every path is
//! beneath the workspace root (or the app's temp folder, where FilmCraft keeps render previews and
//! the MCP bridge writes screenshots): relative paths are taken from the root; absolute paths
//! outside it, `..` escapes and symlinks that lead out are refused with `PermissionDenied`. The
//! same rule holds for the UI and for agents, as with PhotoCraft's `AuthorizedWorkspace` in
//! ImageLab.
//!
//! Some engine paths (an export streaming to its file) open files directly rather than through
//! `Services`; the core makes the workspace the process's working directory so relative paths
//! land there too, and Android's app sandbox bounds the rest.

use std::io;
use std::path::{Component, Path, PathBuf};

use filmcraft_engine::{FsServices, Services, media_browser};

pub struct WorkspaceServices {
    root: PathBuf,
    temp: Option<PathBuf>,
    fs: FsServices,
}

impl WorkspaceServices {
    /// `root` and `temp` must already be canonical.
    pub fn new(root: PathBuf, temp: Option<PathBuf>) -> Self {
        Self { root, temp, fs: FsServices }
    }

    /// The path the engine means, if it is allowed: absolute, beneath the root or temp folder,
    /// symlinks resolved as far as the path exists.
    pub fn resolve(&self, path: &str) -> io::Result<PathBuf> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty path"));
        }
        let p = Path::new(trimmed);
        let joined = if p.is_absolute() { p.to_path_buf() } else { self.root.join(p) };
        // Lexically normalise (no `..` above the start), then resolve the longest existing prefix.
        let mut norm = PathBuf::new();
        for c in joined.components() {
            match c {
                Component::ParentDir => {
                    if !norm.pop() {
                        return Err(denied(trimmed));
                    }
                }
                Component::CurDir => {}
                other => norm.push(other.as_os_str()),
            }
        }
        let resolved = canonical_prefix(&norm)?;
        let inside = resolved.starts_with(&self.root) || self.temp.as_ref().is_some_and(|t| resolved.starts_with(t));
        if inside { Ok(resolved) } else { Err(denied(trimmed)) }
    }

    fn with<R>(&self, path: &str, f: impl FnOnce(&str) -> io::Result<R>) -> io::Result<R> {
        let p = self.resolve(path)?;
        let s = p.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not UTF-8"))?;
        f(s)
    }
}

fn denied(path: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, format!("{path} is outside the VideoLab workspace"))
}

/// `p` with its longest existing ancestor canonicalized (symlinks resolved) and the rest appended.
fn canonical_prefix(p: &Path) -> io::Result<PathBuf> {
    let mut existing = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(c) => {
                let mut out = c;
                for part in rest.iter().rev() {
                    out.push(part);
                }
                return Ok(out);
            }
            Err(_) => {
                let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} has no existing parent", p.display())));
                };
                rest.push(name);
                if !existing.pop() {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} has no existing parent", p.display())));
                }
            }
        }
    }
}

impl Services for WorkspaceServices {
    fn read_file(&self, path: &str) -> io::Result<Vec<u8>> {
        self.with(path, |p| self.fs.read_file(p))
    }
    fn write_file(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.with(path, |p| self.fs.write_file(p, data))
    }
    fn write_file_new(&self, path: &str, data: &[u8]) -> io::Result<()> {
        self.with(path, |p| self.fs.write_file_new(p, data))
    }
    fn file_size(&self, path: &str) -> io::Result<u64> {
        self.with(path, |p| self.fs.file_size(p))
    }
    fn read_range(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.with(path, |p| self.fs.read_range(p, offset, len))
    }
    fn reader(&self, path: &str) -> Option<io::Result<filmcraft_media::SharedReader>> {
        match self.resolve(path) {
            Ok(p) => self.fs.reader(&p.to_string_lossy()),
            Err(e) => Some(Err(e)),
        }
    }
    fn list_dir(&self, dir: &str) -> Option<io::Result<Vec<String>>> {
        let dir = if dir.trim().is_empty() { "." } else { dir };
        match self.resolve(dir) {
            Ok(p) => self.fs.list_dir(&p.to_string_lossy()),
            Err(e) => Some(Err(e)),
        }
    }
    fn file_loader(&self) -> Option<filmcraft_media::sequence::FrameLoader> {
        let root = self.root.clone();
        let temp = self.temp.clone();
        Some(std::sync::Arc::new(move |p: &str| {
            let me = WorkspaceServices::new(root.clone(), temp.clone());
            me.read_file(p)
        }))
    }
    fn list_entries(&self, dir: &str) -> Option<io::Result<Vec<media_browser::DirEntry>>> {
        match self.resolve(dir) {
            Ok(p) => self.fs.list_entries(&p.to_string_lossy()),
            Err(e) => Some(Err(e)),
        }
    }
    /// The Media Browser sees the workspace as its one volume.
    fn volumes(&self) -> Vec<media_browser::Volume> {
        Vec::new()
    }
    fn home_dir(&self) -> Option<String> {
        Some(self.root.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confines_paths_to_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("media")).unwrap();
        std::fs::write(root.join("media/a.txt"), b"hi").unwrap();
        let s = WorkspaceServices::new(root.clone(), None);
        assert_eq!(s.read_file("media/a.txt").unwrap(), b"hi");
        assert_eq!(s.read_file(&root.join("media/a.txt").to_string_lossy()).unwrap(), b"hi");
        assert!(s.resolve("exports/new/out.mp4").unwrap().starts_with(&root));
        for bad in ["../x", "/etc/passwd", "media/../../x", ""] {
            assert!(s.resolve(bad).is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
            assert_eq!(s.resolve("escape/hosts").unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
        std::fs::create_dir_all(root.join("exports")).unwrap();
        s.write_file("exports/out.bin", b"x").unwrap();
        assert!(root.join("exports/out.bin").is_file());
    }
}
