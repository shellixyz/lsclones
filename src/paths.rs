use std::{
    collections::HashSet,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crossterm::cursor;
use derive_more::{Deref, IntoIterator};
use getset::CopyGetters;
use itertools::Itertools;
use num_format::{Locale, ToFormattedString};
use scopeguard::defer;
use size::Size;

use crate::{
    call_rate_limiter::CallRateLimiter,
    clones::db::{ClonesDB, DirCloneFilesStats, PartitionedDirClones},
    error_behavior::ErrorBehavior,
    fs,
    path::HashedAbsolutePathRef,
};

#[derive(Debug, Clone, Deref, Default)]
pub struct Paths(Vec<PathBuf>);

impl Paths {
    pub fn new(inner: Vec<PathBuf>) -> Self {
        Self(inner)
    }
    pub fn into_set(self) -> PathSet {
        self.into()
    }
}

impl IntoIterator for Paths {
    type Item = PathBuf;

    type IntoIter = std::vec::IntoIter<PathBuf>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Paths {
    type Item = &'a Path;

    type IntoIter = PathsIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        PathsIter(self.0.iter())
    }
}

pub struct PathsIter<'a>(std::slice::Iter<'a, PathBuf>);

impl<'a> Iterator for PathsIter<'a> {
    type Item = &'a Path;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.0.next()?.as_path())
    }
}

impl From<PathBuf> for Paths {
    fn from(path_buf: PathBuf) -> Self {
        Self(vec![path_buf])
    }
}

impl From<&PathBuf> for Paths {
    fn from(path_buf_ref: &PathBuf) -> Self {
        Self(vec![path_buf_ref.clone()])
    }
}

impl From<&Vec<PathBuf>> for Paths {
    fn from(path_buf_vec: &Vec<PathBuf>) -> Self {
        Self(path_buf_vec.clone())
    }
}

impl From<&[PathBuf]> for Paths {
    fn from(path_buf_vec: &[PathBuf]) -> Self {
        Self(path_buf_vec.to_vec())
    }
}

impl FromIterator<PathBuf> for Paths {
    fn from_iter<T: IntoIterator<Item = PathBuf>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl From<PathRefs<'_>> for Paths {
    fn from(path_refs: PathRefs) -> Self {
        Self::from_iter(path_refs.into_iter().map(ToOwned::to_owned))
    }
}

#[derive(Debug, Clone, Deref, Default)]
pub struct PathRefs<'a>(Vec<&'a Path>);

impl<'a> PathRefs<'a> {
    pub fn new(inner: Vec<&'a Path>) -> Self {
        Self(inner)
    }

    pub fn into_set(self) -> PathRefSet<'a> {
        self.into()
    }

    // pub fn to_paths(&self) -> Paths {
    //     Paths(self.iter().cloned().map(ToOwned::to_owned).collect())
    // }
}

impl<'a> From<&'a Path> for PathRefs<'a> {
    fn from(path_buf: &'a Path) -> Self {
        Self(vec![path_buf])
    }
}

impl<'a> From<&'a Vec<&'a Path>> for PathRefs<'a> {
    fn from(path_buf_ref_vec: &Vec<&'a Path>) -> Self {
        Self(path_buf_ref_vec.to_vec())
    }
}

impl<'a> From<Vec<&'a Path>> for PathRefs<'a> {
    fn from(path_buf_ref_vec: Vec<&'a Path>) -> Self {
        Self(path_buf_ref_vec)
    }
}

impl<'a> FromIterator<&'a Path> for PathRefs<'a> {
    fn from_iter<T: IntoIterator<Item = &'a Path>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> FromIterator<&'a HashedAbsolutePathRef<'a>> for PathRefs<'a> {
    fn from_iter<T: IntoIterator<Item = &'a HashedAbsolutePathRef<'a>>>(iter: T) -> Self {
        Self(iter.into_iter().map(|hapr| hapr.as_ref()).collect())
    }
}

impl<'a> From<PathRefSet<'a>> for PathRefs<'a> {
    fn from(path_ref_set: PathRefSet<'a>) -> Self {
        Self::from_iter(path_ref_set.iter().copied())
    }
}

impl<'a> IntoIterator for PathRefs<'a> {
    type Item = &'a Path;

    type IntoIter = std::vec::IntoIter<&'a Path>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a PathRefs<'a> {
    type Item = &'a Path;

    type IntoIter = std::iter::Copied<std::slice::Iter<'a, &'a Path>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().copied()
    }
}

#[derive(Debug, Deref, IntoIterator)]
pub struct PathSet(HashSet<PathBuf>);

impl PathSet {
    pub fn to_paths(&self) -> Paths {
        Paths::new(Vec::from_iter(self.iter().cloned()))
    }
}

impl FromIterator<PathBuf> for PathSet {
    fn from_iter<T: IntoIterator<Item = PathBuf>>(iter: T) -> Self {
        Self(HashSet::from_iter(iter))
    }
}

// impl FromIterator<&PathBuf> for PathSet {
//     fn from_iter<T: IntoIterator<Item = PathBuf>>(iter: T) -> Self {
//         Self(HashSet::from_iter(iter))
//     }
// }

impl From<Paths> for PathSet {
    fn from(paths: Paths) -> Self {
        Self::from_iter(paths)
    }
}

#[derive(Debug, Deref)]
pub struct PathRefSet<'a>(HashSet<&'a Path>);

impl<'a> PathRefSet<'a> {
    pub fn into_path_refs(self) -> PathRefs<'a> {
        self.into()
    }
}

impl<'a> FromIterator<&'a Path> for PathRefSet<'a> {
    fn from_iter<T: IntoIterator<Item = &'a Path>>(iter: T) -> Self {
        Self(HashSet::from_iter(iter))
    }
}

impl<'a> From<PathRefs<'a>> for PathRefSet<'a> {
    fn from(path_refs: PathRefs<'a>) -> Self {
        Self::from_iter(path_refs)
    }
}

impl<'a> AsRef<PathRefs<'a>> for PathRefs<'a> {
    fn as_ref(&self) -> &PathRefs<'a> {
        self
    }
}

pub trait TreeWithProgress<'a> {
    fn tree_with_progress(&'a self, error_behavior: ErrorBehavior) -> anyhow::Result<fs::Tree>;
}

impl<'a, T> TreeWithProgress<'a> for T
where
    T: 'a,
    &'a T: IntoIterator<Item = &'a Path>,
{
    fn tree_with_progress(&'a self, error_behavior: ErrorBehavior) -> anyhow::Result<fs::Tree> {
        let mut tree = fs::Tree::default();
        let (mut total_dir_count, mut total_file_count) = (0, 0);
        defer!(crossterm::execute!(io::stderr(), cursor::Show).unwrap());
        crossterm::execute!(io::stderr(), cursor::Hide).unwrap();
        let progress_func = |(dirs, files): (u64, u64)| {
            bunt::eprint!(
                "\r{$green}INFO{/$}  {$bold}>{/$} Listing files: {} dirs - {} files",
                dirs.to_formatted_string(&Locale::en),
                files.to_formatted_string(&Locale::en)
            );
            io::stderr().flush().unwrap();
        };
        let mut progress_display = CallRateLimiter::new(0.1, progress_func);
        for path in self.into_iter() {
            let (dir_count, file_count) =
                tree.extend_with_progress(path, error_behavior, |dir_count, file_count| {
                    progress_display.call((dir_count, file_count))
                })?;
            total_dir_count += dir_count;
            total_file_count += file_count;
        }
        progress_display.call_unconditional((total_dir_count, total_file_count));
        eprintln!();
        Ok(tree)
    }
}

#[derive(Debug, Clone, Copy, Default, CopyGetters)]
#[getset(get_copy = "pub")]
pub struct CloneStats {
    total_count: usize,
    total_size: u64,
    reclaimable_count: usize,
    reclaimable_size: u64,
}

impl CloneStats {
    pub fn new(
        total_count: usize,
        total_size: u64,
        reclaimable_count: usize,
        reclaimable_size: u64,
    ) -> Self {
        Self {
            total_count,
            total_size,
            reclaimable_count,
            reclaimable_size,
        }
    }

    pub fn total_size_human(&self) -> Size {
        Size::from_bytes(self.total_size)
    }

    pub fn reclaimable_size_human(&self) -> Size {
        Size::from_bytes(self.reclaimable_size)
    }
}

pub trait Clones<'a> {
    fn clones(
        &'a self,
        recursive: bool,
        clones_db: &'a ClonesDB,
    ) -> (DirCloneFilesStats, PathRefs<'a>);
    fn inside_clones(
        &'a self,
        recursive: bool,
        clones_db: &'a ClonesDB,
    ) -> (CloneStats, PathRefs<'a>);
    fn clone_groups(
        &'a self,
        recursive: bool,
        clones_db: &'a ClonesDB,
    ) -> Vec<PartitionedDirClones<'a>>;
}

impl<'a, T> Clones<'a> for T
where
    T: 'a,
    &'a T: IntoIterator<Item = &'a Path>,
{
    fn clones(
        &'a self,
        recursive: bool,
        clones_db: &'a ClonesDB,
    ) -> (DirCloneFilesStats, PathRefs<'a>) {
        let mut stats = DirCloneFilesStats::default();
        let clones = self
            .into_iter()
            .flat_map(|path| {
                if path.is_dir() {
                    let clones = clones_db.dir_clone_files(path, recursive);
                    stats += *clones.stats();
                    clones.into_iter().map(|c| c.inner()).collect()
                } else if path.is_file() && clones_db.file_is_a_clone(path) {
                    path.into()
                } else {
                    PathRefs::default()
                }
            })
            .unique()
            .collect();
        (stats, clones)
    }

    fn inside_clones(
        &'a self,
        recursive: bool,
        clones_db: &'a ClonesDB,
    ) -> (CloneStats, PathRefs<'a>) {
        let mut stats = CloneStats::default();
        let clones = self
            .clone_groups(recursive, clones_db)
            .iter()
            .filter(|&group| (group.inside().len() > 1))
            .flat_map(|group| {
                let inside = group.inside();
                stats.total_count += inside.total_count();
                stats.total_size += inside.total_size();
                stats.reclaimable_count += inside.reclaimable_count();
                stats.reclaimable_size += inside.reclaimable_size();
                inside.iter().map(|path| path.inner()).collect_vec()
            })
            .collect();
        (stats, clones)
    }

    fn clone_groups(
        &'a self,
        recursive: bool,
        clones_db: &'a ClonesDB,
    ) -> Vec<PartitionedDirClones<'a>> {
        self.into_iter()
            .filter(|path| path.is_dir())
            .flat_map(|dir| clones_db.dir_clone_groups(dir, recursive))
            .collect()
    }
}
