
use std::{path::{PathBuf, Path}, fmt::Display, collections::BTreeSet, hash::Hash};

use anyhow::anyhow;
use derive_more::{Deref, DerefMut, IntoIterator};
use getset::{CopyGetters, Getters};
use path_absolutize::Absolutize;

use crate::hash::HashValue;

fn parent_and_comps_hashes(path: impl AsRef<Path>) -> (Option<u64>, Vec<u64>) {
    let comps = path.as_ref().components()
        .fold(vec![], |mut comps, comp| {
            match comps.last() {
                None => comps.push(PathBuf::from(comp.as_os_str())),
                Some(last) => comps.push(last.join(comp.as_os_str())),
            }
            comps
        });

    let mut comp_hashes = comps.iter().skip(1).map(HashValue::hash_value).collect::<Vec<_>>();
    let parent_hash = if comp_hashes.len() > 1 { Some(comp_hashes[comp_hashes.len() - 2]) } else { None };
    comp_hashes.sort();
    (parent_hash, comp_hashes)
}

pub trait HashedPath {
    fn path_hash(&self) -> u64;
}

pub trait AsPath {
    fn as_path(&self) -> &Path;
}

pub trait HashedParentCompsPath {
    fn comp_hashes(&self) -> &Vec<u64>;
}

#[derive(Debug, Clone, Eq, Deref, CopyGetters, Getters)]
pub struct HashedAbsolutePath {
    #[deref]
    path: PathBuf,
    #[getset(get_copy = "pub")]
    hash: u64,
    #[getset(get_copy = "pub")]
    parent_hash: Option<u64>,
    #[getset(get = "pub")]
    comp_hashes: Vec<u64>,
}

// impl<T: AsRef<Path>> From<T> for HashedAbsolutePath {
//     fn from(into_path_buf: T) -> Self {
//         let path = into_path_buf.as_ref().absolutize().unwrap().to_path_buf();
//         let hash = path.hash_value();
//         let (parent_hash, comp_hashes) = parent_and_comps_hashes(&path);
//         Self { path, hash, parent_hash, comp_hashes }
//     }
// }

impl From<PathBuf> for HashedAbsolutePath {
    fn from(path_buf: PathBuf) -> Self {
        let path = path_buf.absolutize().unwrap().to_path_buf();
        let hash = path.hash_value();
        let (parent_hash, comp_hashes) = parent_and_comps_hashes(&path);
        Self { path, hash, parent_hash, comp_hashes }
    }
}

impl From<&str> for HashedAbsolutePath {
    fn from(str: &str) -> Self {
        PathBuf::from(str).into()
    }
}

impl From<&String> for HashedAbsolutePath {
    fn from(str: &String) -> Self {
        PathBuf::from(str).into()
    }
}

impl From<&Path> for HashedAbsolutePath {
    fn from(path: &Path) -> Self {
        path.to_path_buf().into()
    }
}

impl HashedAbsolutePath {

    pub fn to_absolute_path_ref(&self) -> HashedAbsolutePathRef {
        HashedAbsolutePathRef::from(self)
    }

    pub fn starts_with(&self, path: impl AsRef<Path>) -> anyhow::Result<bool> {
        let path = path.as_ref();
        if ! path.is_absolute() {
            return Err(anyhow!("path is not absolute: {}", path.to_string_lossy()))
        }
        if path.as_os_str() == "/" { return Ok(true); }
        Ok(self.comp_hash_matches(path.hash_value()))
    }

    pub fn starts_with_hap(&self, path: impl AsRef<HashedAbsolutePath>) -> bool {
        let path = path.as_ref();
        if path.as_os_str() == "/" { return true; }
        self.comp_hash_matches(path.hash)
    }

    pub fn comp_hash_matches(&self, comp_hash: u64) -> bool {
        self.comp_hashes().binary_search(&comp_hash).is_ok()
    }

    pub fn parent_is(&self, path: impl AsRef<Path>) -> anyhow::Result<bool> {
        let path = path.as_ref();
        if ! path.is_absolute() {
            return Err(anyhow!("path is not absolute: {}", path.to_string_lossy()))
        }
        match &self.parent_hash {
            Some(parent_hash) => Ok(*parent_hash == path.hash_value()),
            None => Ok(path.as_os_str() == "/"),
        }

    }

    pub fn parent_is_hap(&self, path: impl AsRef<Self>) -> bool {
        match &self.parent_hash {
            Some(parent_hash) => *parent_hash == path.as_ref().hash(),
            None => path.as_ref().as_os_str() == "/",
        }
    }

}

impl AsRef<HashedAbsolutePath> for HashedAbsolutePath {
    fn as_ref(&self) -> &HashedAbsolutePath {
        self
    }
}

impl AsRef<Path> for HashedAbsolutePath {
    fn as_ref(&self) -> &Path {
        self.path.as_path()
    }
}

impl HashedPath for HashedAbsolutePath {
    fn path_hash(&self) -> u64 {
        self.hash
    }
}

impl Hash for HashedAbsolutePath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

impl AsPath for HashedAbsolutePath {
    fn as_path(&self) -> &Path {
        self.path.as_path()
    }
}

impl HashedParentCompsPath for HashedAbsolutePath {
    fn comp_hashes(&self) -> &Vec<u64> {
        &self.comp_hashes
    }
}

impl PartialEq for HashedAbsolutePath {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl PartialOrd for HashedAbsolutePath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.path.partial_cmp(&other.path)
    }
}

impl Ord for HashedAbsolutePath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path.cmp(&other.path)
    }
}

impl Display for HashedAbsolutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path.to_string_lossy())
    }
}

#[derive(Debug, Clone, Eq, Deref, CopyGetters, Getters)]
pub struct HashedAbsolutePathRef<'a> {
    #[deref]
    path: &'a Path,
    #[getset(get_copy = "pub")]
    hash: u64,
    #[getset(get_copy = "pub")]
    parent_hash: Option<u64>,
    #[getset(get = "pub")]
    comp_hashes: Vec<u64>,
}

impl<'a> HashedAbsolutePathRef<'a> {

    pub fn new(path: &'a Path) -> anyhow::Result<Self> {
        if ! path.is_absolute() { return Err(anyhow!("path is not absolute: {}", path.to_string_lossy())) }
        let hash = path.hash_value();
        let (parent_hash, comp_hashes) = parent_and_comps_hashes(path);
        Ok(Self { path, hash, parent_hash, comp_hashes })
    }

    pub fn inner(&self) -> &'a Path {
        self.path
    }

    pub fn starts_with(&self, path: impl AsRef<Path>) -> anyhow::Result<bool> {
        let path = path.as_ref();
        if ! path.is_absolute() {
            return Err(anyhow!("path is not absolute: {}", path.to_string_lossy()))
        }
        if path.as_os_str() == "/" { return Ok(true); }
        Ok(self.comp_hash_matches(path.hash_value()))
    }

    pub fn starts_with_hashed_path(&self, path: impl AsRef<HashedAbsolutePath>) -> bool {
        let path = path.as_ref();
        if path.as_os_str() == "/" { return true; }
        self.comp_hash_matches(path.hash)
    }

    pub fn comp_hash_matches(&self, comp_hash: u64) -> bool {
        self.comp_hashes().binary_search(&comp_hash).is_ok()
    }

    pub fn parent_is(&self, path: impl AsRef<Path>) -> anyhow::Result<bool> {
        let path = path.as_ref();
        if ! path.is_absolute() {
            return Err(anyhow!("path is not absolute: {}", path.to_string_lossy()))
        }
        match &self.parent_hash {
            Some(parent_hash) => Ok(*parent_hash == path.hash_value()),
            None => Ok(false),
        }
    }

    pub fn parent_is_hap(&self, path: impl AsRef<HashedAbsolutePath>) -> bool {
        match &self.parent_hash {
            Some(parent_hash) => *parent_hash == path.as_ref().hash(),
            None => path.as_ref().as_os_str() == "/",
        }
    }

}

impl<'a> AsRef<HashedAbsolutePathRef<'a>> for HashedAbsolutePathRef<'a> {
    fn as_ref(&self) -> &HashedAbsolutePathRef<'a> {
        self
    }
}

impl<'a> AsRef<Path> for HashedAbsolutePathRef<'a> {
    fn as_ref(&self) -> &Path {
        self.path
    }
}

impl<'a> TryFrom<&'a Path> for HashedAbsolutePathRef<'a> {
    type Error = anyhow::Error;

    fn try_from(path: &'a Path) -> Result<Self, Self::Error> {
        if ! path.is_absolute() { return Err(anyhow!("path is not absolute: {}", path.to_string_lossy())) }
        let hash = path.hash_value();
        let (parent_hash, comp_hashes) = parent_and_comps_hashes(path);
        Ok(Self { path, hash, parent_hash, comp_hashes })
    }
}

impl<'a> From<&'a HashedAbsolutePath> for HashedAbsolutePathRef<'a> {
    fn from(hashed_abs_path: &'a HashedAbsolutePath) -> Self {
        HashedAbsolutePathRef {
            path: &hashed_abs_path.path,
            hash: hashed_abs_path.hash,
            parent_hash: hashed_abs_path.parent_hash,
            comp_hashes: hashed_abs_path.comp_hashes.clone(),
        }
    }
}

impl<'a> Hash for HashedAbsolutePathRef<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

impl<'a> HashedPath for HashedAbsolutePathRef<'a> {
    fn path_hash(&self) -> u64 {
        self.hash
    }
}

impl<'a> AsPath for HashedAbsolutePathRef<'a> {
    fn as_path(&self) -> &Path {
        self.path
    }
}

impl<'a> HashedParentCompsPath for HashedAbsolutePathRef<'a> {
    fn comp_hashes(&self) -> &Vec<u64> {
        &self.comp_hashes
    }
}

impl<'a> PartialEq for HashedAbsolutePathRef<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl<'a> PartialOrd for HashedAbsolutePathRef<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.path.partial_cmp(other.path)
    }
}

impl<'a> Ord for HashedAbsolutePathRef<'a> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path.cmp(other.path)
    }
}

impl<'a> Display for HashedAbsolutePathRef<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path.to_string_lossy())
    }
}

#[derive(Debug, Deref, DerefMut, Default)]
pub struct HashedAbsolutePathSet(BTreeSet<HashedAbsolutePath>);

impl HashedAbsolutePathSet {

    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains_parent_of(&self, base: impl AsRef<Path>) -> anyhow::Result<bool> {
        let base = base.as_ref();
        if ! base.is_absolute() {
            return Err(anyhow!("path is not absolute: {}", base.to_string_lossy()))
        }
        Ok(self.iter().any(|path| path.starts_with(base).unwrap()))
    }
}

impl Clone for HashedAbsolutePathSet {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl FromIterator<PathBuf> for HashedAbsolutePathSet {
    fn from_iter<T: IntoIterator<Item = PathBuf>>(iter: T) -> Self {
        Self(BTreeSet::from_iter(iter.into_iter().map(HashedAbsolutePath::from)))
    }
}

impl FromIterator<HashedAbsolutePath> for HashedAbsolutePathSet {
    fn from_iter<T: IntoIterator<Item = HashedAbsolutePath>>(iter: T) -> Self {
        Self(BTreeSet::from_iter(iter.into_iter()))
    }
}

impl From<&[PathBuf]> for HashedAbsolutePathSet {
    fn from(path_buf_slice: &[PathBuf]) -> Self {
        Self::from_iter(path_buf_slice.iter().cloned())
    }
}

#[derive(Debug, Deref, DerefMut, IntoIterator, Default)]
pub struct HashedAbsolutePathRefSet<'a>(BTreeSet<HashedAbsolutePathRef<'a>>);

impl<'a> Clone for HashedAbsolutePathRefSet<'a> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<'a> FromIterator<HashedAbsolutePathRef<'a>> for HashedAbsolutePathRefSet<'a> {
    fn from_iter<T: IntoIterator<Item = HashedAbsolutePathRef<'a>>>(iter: T) -> Self {
        Self(BTreeSet::from_iter(iter.into_iter()))
    }
}

impl<'a> FromIterator<HashedAbsolutePathRefSet<'a>> for HashedAbsolutePathRefSet<'a> {
    fn from_iter<T: IntoIterator<Item = HashedAbsolutePathRefSet<'a>>>(iter: T) -> Self {
        let mut set = Self::default();
        for iset in iter {
            for path in iset {
                set.insert(path);
            }
        }
        set
    }
}

impl<'a> IntoIterator for &'a HashedAbsolutePathRefSet<'a> {
    type Item = &'a HashedAbsolutePathRef<'a>;

    type IntoIter = std::collections::btree_set::Iter<'a, HashedAbsolutePathRef<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}


#[cfg(test)]
mod tests {

    mod hashed_absolute_path {
        use crate::path::HashedAbsolutePath;

        #[test]
        fn starts_with() {
            let x = HashedAbsolutePath::from("/a/b/c");
            let y = HashedAbsolutePath::from("/a");
            let z = HashedAbsolutePath::from("/b");
            assert!(x.starts_with_hap(&y));
            assert!(x.starts_with(y.as_path()).unwrap());
            assert!(!x.starts_with(z.as_path()).unwrap());
        }

        #[test]
        fn is_parent() {
            let x = HashedAbsolutePath::from("/a/b/c");
            let y = HashedAbsolutePath::from("/a/b");
            let z = HashedAbsolutePath::from("/b");
            assert!(x.parent_is_hap(&y));
            assert!(x.parent_is(y.as_path()).unwrap());
            assert!(!x.parent_is_hap(&z));
            assert!(!x.parent_is(z.as_path()).unwrap());
        }

        #[test]
        fn eq() {
            let x = HashedAbsolutePath::from("/a/b/c");
            let y = HashedAbsolutePath::from("/a/b/c");
            let z = HashedAbsolutePath::from("/b");
            assert_eq!(x, y);
            assert_eq!(y, x);
            assert_ne!(x, z);
            assert_ne!(z, x);
        }

        #[test]
        fn ord() {
            let x = HashedAbsolutePath::from("/a/b/cde");
            let y = HashedAbsolutePath::from("/a/b/def");
            assert!(x < y);
            assert!(y > x);
        }

    }
}