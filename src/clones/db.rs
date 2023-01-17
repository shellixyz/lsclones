
use std::{collections::{hash_map, BTreeMap, HashMap}, path::{Path, self}, fmt::Debug, io::{self, Write}};

use crossterm::cursor;
use derive_more::{Deref, Constructor, IntoIterator, Add, AddAssign};
use getset::{Getters, CopyGetters};
use itertools::Itertools;
use ouroboros::self_referencing;
use path_absolutize::Absolutize;
use scopeguard::defer;
use size::Size;
use num_format::{ToFormattedString, Locale};

use crate::{path::{HashedAbsolutePathSet, HashedAbsolutePath, HashedAbsolutePathRefSet, HashedAbsolutePathRef}, call_rate_limiter::CallRateLimiter};

use super::File;


pub trait CloneGroupFileCountAndSize {
    fn file_size(&self) -> u64;
    fn total_count(&self) -> usize;
    fn total_size(&self) -> u64;
}

pub trait CloneGroupReclaimable {
    fn reclaimable_count(&self) -> usize;
    fn reclaimable_size(&self) -> u64;
}

#[derive(Debug, Deref, CopyGetters, Getters)]
pub struct CloneGroup {
    #[getset(get_copy = "pub")]
    file_size: u64,
    #[deref] #[getset(get = "pub")]
    files: HashedAbsolutePathSet,
}

impl CloneGroupFileCountAndSize for CloneGroup {
    fn file_size(&self) -> u64 {
        self.file_size
    }

    fn total_count(&self) -> usize {
        self.files.len()
    }

    fn total_size(&self) -> u64 {
        self.files.len() as u64 * self.file_size
    }
}

impl CloneGroup {
    pub fn from_parts(file_size: u64, files: HashedAbsolutePathSet) -> anyhow::Result<Self> {
        Ok(Self { file_size, files })
    }
}

#[derive(Debug, Deref, CopyGetters, Getters, Clone)]
pub struct CloneRefGroup<'a> {
    #[getset(get_copy = "pub")]
    file_size: u64,
    #[deref] #[getset(get = "pub")]
    files: HashedAbsolutePathRefSet<'a>
}

impl<'a> CloneGroupFileCountAndSize for CloneRefGroup<'a> {
    fn file_size(&self) -> u64 {
        self.file_size
    }

    fn total_count(&self) -> usize {
        self.files.len()
    }

    fn total_size(&self) -> u64 {
        self.files.len() as u64 * self.file_size
    }
}

impl<'a> CloneRefGroup<'a> {

    pub fn from_parts(file_size: u64, files: HashedAbsolutePathRefSet<'a>) -> Self {
        Self { file_size, files }
    }

    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    pub fn total_size(&self) -> u64 {
        self.total_count() as u64 * self.file_size
    }

    pub fn reclaimable_count(&self) -> usize {
        if self.is_empty() { return 0; }
        self.files.len() - 1
    }

    pub fn reclaimable_size(&self) -> u64 {
        self.reclaimable_count() as u64 * self.file_size
    }

    pub fn filter_out_dir(&self, dir: impl AsRef<Path>) -> CloneRefGroup {
        let dir = HashedAbsolutePath::from(dir.as_ref());
        let files = self.files.iter().filter(|ifile|
            !ifile.starts_with_hashed_path(&dir)
        ).cloned().collect();
        CloneRefGroup::from_parts(self.file_size, files)
    }

}

impl<T: CloneGroupFileCountAndSize> CloneGroupReclaimable for T {
    fn reclaimable_count(&self) -> usize {
        if self.total_count() == 0 { return 0; }
        self.total_count() - 1
    }

    fn reclaimable_size(&self) -> u64 {
        self.reclaimable_count() as u64 * self.file_size()
    }
}

#[derive(Debug, CopyGetters, Deref, Constructor)]
pub struct FileClones<'a> {
    #[getset(get_copy = "pub")]
    file_size: u64,
    #[deref]
    clones: HashedAbsolutePathRefSet<'a>
}

impl<'a> FileClones<'a> {
    pub fn reclaimable_size(&self) -> u64 {
        self.clones.len() as u64 * self.file_size
    }
}

#[self_referencing]
pub struct CloneGroups {
    groups: Vec<CloneGroup>,
    #[borrows(groups)] #[covariant]
    ref_groups: Vec<CloneRefGroup<'this>>,
    #[borrows(ref_groups)] #[covariant]
    files: BTreeMap<&'this path::Path, &'this CloneRefGroup<'this>>,
}

impl Debug for CloneGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloneGroups")
            .field("groups", self.borrow_groups())
            .finish()
    }
}

pub struct CloneFilesIter<'a> {
    groups_iter: std::slice::Iter<'a, CloneRefGroup<'a>>,
    group_iter: Option<std::collections::btree_set::Iter<'a, HashedAbsolutePathRef<'a>>>,
}

impl<'a> Iterator for CloneFilesIter<'a> {
    type Item = &'a HashedAbsolutePathRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(group_iter) = &mut self.group_iter {
                if let Some(file) = group_iter.next() {
                    return Some(file);
                }
            }

            if let Some(group) = self.groups_iter.next() {
                self.group_iter = Some(group.iter());
            } else {
                return None;
            }
        }
    }
}

pub struct DirCloneFilesIter<'a> {
    files_iter: CloneFilesIter<'a>,
    dir_hash: Option<u64>,
    recursive: bool,
}

impl<'a> Iterator for DirCloneFilesIter<'a> {
    type Item = &'a HashedAbsolutePathRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.files_iter.find(|file| {
            if self.recursive {
                file.comp_hash_matches(self.dir_hash.unwrap())
            } else {
                file.parent_hash() == self.dir_hash
            }
        })
    }
}

pub enum PathClonesIter<'a> {
    File(FileClonesIter<'a>),
    Dir(DirCloneFilesIter<'a>),
    Other,
}

impl<'a> Iterator for PathClonesIter<'a> {
    type Item = &'a HashedAbsolutePathRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        use PathClonesIter::*;
        match self {
            File(iter) => iter.next(),
            Dir(iter) => iter.next(),
            Other => None,
        }
    }
}

pub struct PathsClonesIter<'a> {
    clone_groups: &'a CloneGroups,
    paths: Vec<HashedAbsolutePath>,
    path_index: usize,
    path_clones_iter: Option<PathClonesIter<'a>>,
    recursive: bool,
    returned_hashes: HashMap<u64, ()>,
}

impl<'a> Iterator for PathsClonesIter<'a> {
    type Item = &'a HashedAbsolutePathRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(path_clones_iter) = &mut self.path_clones_iter {
                if let Some(clone) = path_clones_iter.next() {
                    if let hash_map::Entry::Vacant(entry) = self.returned_hashes.entry(clone.hash()) {
                        entry.insert(());
                        return Some(clone);
                    } else {
                        continue; // item was already returned, continue consuming path_clones_iter
                    }
                }
            }

            // path_clones_iter is None or next() returned None
            if let Some(path) = self.paths.get(self.path_index) {
                self.path_clones_iter = Some(self.clone_groups.path_clones_iter_hap(path, self.recursive));
                self.path_index += 1;
            } else {
                return None;
            }
        }
    }
}

pub struct FileClonesIter<'a> {
    iter: Option<std::collections::btree_set::Iter<'a, HashedAbsolutePathRef<'a>>>,
    orig_file_hash: u64,
}

impl<'a> Iterator for FileClonesIter<'a> {
    type Item = &'a HashedAbsolutePathRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.iter {
            Some(iter) =>
                iter.find(|file| file.hash() != self.orig_file_hash),
            None => None,
        }
    }
}

impl CloneGroups {

    pub fn file_is_a_clone<P: AsRef<Path>>(&self, file: P) -> bool {
        let file = file.as_ref().absolutize().unwrap();//.to_path_buf();
        self.borrow_files().contains_key(file.as_ref())
    }

    pub fn file_clones<P: AsRef<Path>>(&self, file: P) -> Option<FileClones> {
        self.file_clones_hap(HashedAbsolutePath::from(file.as_ref()))
    }

    pub fn clone_group<P: AsRef<Path>>(&self, file: P) -> Option<&CloneRefGroup> {
        self.borrow_files().get(file.as_ref()).copied()
    }

    pub fn file_clones_hap<P: AsRef<HashedAbsolutePath>>(&self, file: P) -> Option<FileClones> {
        let file = file.as_ref();
        let ref_group = self.borrow_files().get(file.as_path())?;
        let clones = ref_group.files.iter().cloned().filter(|ifile|
            *ifile != file.to_absolute_path_ref()
        ).collect();
        Some(FileClones::new(ref_group.file_size, clones))
    }

    pub fn file_clones_iter<P: AsRef<Path>>(&self, file: P) -> FileClonesIter {
        self.file_clones_iter_hap(HashedAbsolutePath::from(file.as_ref()))
    }

    pub fn file_clones_iter_hap<P: AsRef<HashedAbsolutePath>>(&self, file: P) -> FileClonesIter {
        let file = file.as_ref();
        let ref_group = self.borrow_files().get(file.as_path());
        FileClonesIter { iter: ref_group.map(|rg| rg.files.iter()), orig_file_hash: file.hash() }
    }

    pub fn files_iter(&self) -> CloneFilesIter {
        CloneFilesIter { groups_iter: self.borrow_ref_groups().iter(), group_iter: None }
    }

    pub fn dir_clone_files(&self, dir: impl AsRef<Path>, recursive: bool) -> DirCloneFiles {
        self.dir_clone_files_hap(HashedAbsolutePath::from(dir.as_ref()), recursive)
    }

    pub fn dir_clone_files_hap(&self, dir: impl AsRef<HashedAbsolutePath>, recursive: bool) -> DirCloneFiles {
        let dir_clone_groups = self.dir_clone_groups_hap(dir, recursive);
        let clone_files = dir_clone_groups.iter().map(|clones| clones.inside.files.clone()).collect();
        let (total_size, reclaimable_count, reclaimable_size) =
            dir_clone_groups.iter().fold((0, 0, 0), |(t_size, r_count, r_size), clones| {
                (
                    t_size + clones.total_size(),
                    r_count + clones.inside_reclaimable_count(),
                    r_size + clones.inside_reclaimable_size()
                )
            });
        DirCloneFiles {
            stats: DirCloneFilesStats { total_size, reclaimable_count, reclaimable_size },
            clones: clone_files,
        }
    }

    pub fn dir_clone_files_iter<P: AsRef<Path>>(&self, dir: P, recursive: bool) -> DirCloneFilesIter {
        self.dir_clone_files_iter_hap(HashedAbsolutePath::from(dir.as_ref()), recursive)
    }

    pub fn dir_clone_files_iter_hap<P: AsRef<HashedAbsolutePath>>(&self, dir: P, recursive: bool) -> DirCloneFilesIter {
        let dir = dir.as_ref();
        let dir_hash = if dir.as_os_str() == "/" { None } else { Some(dir.hash()) };
        DirCloneFilesIter { files_iter: self.files_iter(), dir_hash, recursive }
    }

    pub fn path_clones_iter(&self, path: impl AsRef<Path>, recursive: bool) -> PathClonesIter {
        self.path_clones_iter_hap(HashedAbsolutePath::from(path.as_ref()), recursive)
    }

    pub fn path_clones_iter_hap(&self, path: impl AsRef<HashedAbsolutePath>, recursive: bool) -> PathClonesIter {
        let path = path.as_ref();
        if path.is_dir() {
            PathClonesIter::Dir(self.dir_clone_files_iter_hap(path, recursive))
        } else if path.is_file() {
            PathClonesIter::File(self.file_clones_iter_hap(path))
        } else {
            PathClonesIter::Other
        }
    }

    pub fn paths_clones_iter<'a>(&self, paths: impl IntoIterator<Item = &'a str>, recursive: bool) -> PathsClonesIter {
        self.paths_clones_iter_hap(paths.into_iter().map(Into::into).collect_vec(), recursive)
    }

    pub fn paths_clones_iter_hap(&self, paths: impl IntoIterator<Item = HashedAbsolutePath>, recursive: bool) -> PathsClonesIter {
        PathsClonesIter {
            clone_groups: self,
            paths: paths.into_iter().collect(),
            path_index: 0,
            path_clones_iter: None,
            recursive,
            returned_hashes: HashMap::new(),
        }
    }

    pub fn dir_clone_groups(&self, dir: impl AsRef<Path>, recursive: bool) -> Vec<PartitionedDirClones> {
        self.dir_clone_groups_hap(HashedAbsolutePath::from(dir.as_ref()), recursive)
    }

    pub fn dir_clone_groups_hap(&self, dir: impl AsRef<HashedAbsolutePath>, recursive: bool) -> Vec<PartitionedDirClones> {
        let ref_groups = self.borrow_ref_groups();
        ref_groups.iter().filter_map(|group| {

            let (inside_dir, outside_dir) =
                group.iter().partition::<Vec<&HashedAbsolutePathRef>, _>(|file|
                    if recursive {
                        file.starts_with_hashed_path(&dir)
                    } else {
                        file.parent_is_hap(&dir)
                    }
                );

            (!inside_dir.is_empty()).then(|| {
                let inside_dir = inside_dir.into_iter().cloned().collect::<HashedAbsolutePathRefSet>();
                let inside_clone_group = CloneRefGroup::from_parts(group.file_size, inside_dir);
                let outside_dir = outside_dir.into_iter().cloned().collect::<HashedAbsolutePathRefSet>();
                let outside_clone_group = CloneRefGroup::from_parts(group.file_size, outside_dir);
                PartitionedDirClones::new(group.file_size, inside_clone_group, outside_clone_group)
            })

        }).collect()
    }

}

#[derive(Debug, Clone, Copy, CopyGetters, Default, Add, AddAssign)]
#[getset(get_copy = "pub")]
pub struct DirCloneFilesStats {
    total_size: u64,
    reclaimable_count: usize,
    reclaimable_size: u64,
}

impl DirCloneFilesStats {

    pub fn total_size_human(&self) -> Size {
        Size::from_bytes(self.total_size)
    }

    pub fn reclaimable_size_human(&self) -> Size {
        Size::from_bytes(self.reclaimable_size)
    }

}

#[derive(Debug, Deref, IntoIterator, Getters)]
pub struct DirCloneFiles<'a> {
    #[getset(get = "pub")]
    stats: DirCloneFilesStats,
    #[deref] #[into_iterator]
    clones: HashedAbsolutePathRefSet<'a>
}

#[derive(Debug, Getters, CopyGetters)]
pub struct PartitionedDirClones<'a> {
    #[getset(get_copy = "pub")]
    file_size: u64,
    #[getset(get = "pub")]
    inside: CloneRefGroup<'a>,
    #[getset(get = "pub")]
    outside: CloneRefGroup<'a>,
}

impl<'a> PartitionedDirClones<'a> {

    pub fn new(file_size: u64, inside: CloneRefGroup<'a>, outside: CloneRefGroup<'a>) -> Self {
        Self { file_size, inside, outside }
    }

    pub fn file_count(&self) -> usize {
        self.inside.len() + self.outside.len()
    }

    pub fn total_size(&self) -> u64 {
        self.file_count() as u64 * self.file_size
    }

    pub fn reclaimable_size(&self) -> u64 {
        let file_count = self.file_count();
        if file_count == 0 { return 0; }
        (file_count as u64 - 1) * self.file_size
    }

    pub fn inside_reclaimable_count(&self) -> usize {
        if self.outside.is_empty() {
            self.inside.reclaimable_count()
        } else {
            self.inside.total_count()
        }
    }

    pub fn inside_reclaimable_size(&self) -> u64 {
        if self.outside.is_empty() {
            self.inside.reclaimable_size()
        } else {
            self.inside.total_size()
        }
    }

}

impl FromIterator<(u64, Vec<HashedAbsolutePath>)> for CloneGroups {
    fn from_iter<T: IntoIterator<Item = (u64, Vec<HashedAbsolutePath>)>>(iter: T) -> Self {
        Self::new(
            iter.into_iter().map(|(file_size, files)| {
                let files = HashedAbsolutePathSet::from_iter(files.into_iter());
                CloneGroup::from_parts(file_size, files).unwrap()
            }).collect(),
            |groups| {
                groups.iter().map(|group| {
                    let files = group.files().iter().map(HashedAbsolutePathRef::from).collect();
                    CloneRefGroup::from_parts(group.file_size, files)
                }).collect()
            },
            |ref_groups| {
                let mut files = BTreeMap::new();
                for group in ref_groups {
                    for file in group.iter() {
                        files.insert(file.as_ref(), group);
                    }
                }
                files
            }
        )
    }
}

impl From<Vec<(u64, Vec<HashedAbsolutePath>)>> for CloneGroups {
    fn from(clone_groups: Vec<(u64, Vec<HashedAbsolutePath>)>) -> Self {
        Self::from_iter(clone_groups.into_iter())
    }
}

#[derive(Debug, Getters, Deref)]
#[getset(get = "pub")]
pub struct ClonesDB {
    scanned_paths: Option<HashedAbsolutePathSet>,
    #[deref]
    clone_groups: CloneGroups,
}

impl ClonesDB {

    /// reads clones database file in json format
    pub fn read_clones_file<P: AsRef<Path>>(path: P, prune: bool) -> anyhow::Result<Self> {
        log::info!("Loading clones list file: {}", path.as_ref().to_string_lossy());
        let file = File::open(path)?;

        if prune {
            defer!(crossterm::execute!(io::stderr(), cursor::Show).unwrap());
            crossterm::execute!(io::stderr(), cursor::Hide).unwrap();
        }

        let mut scanned_paths = file.scanned_paths()?;
        if prune && scanned_paths.is_some() {
            let progress_func = |(index, total): (usize, usize)| {
                let percent = index * 100 / total;
                bunt::eprint!("\r{$green}INFO{/$}  {$bold}>{/$} Pruning scanned dirs list {} / {} ({}%)", index.to_formatted_string(&Locale::en), total.to_formatted_string(&Locale::en), percent);
                io::stderr().flush().unwrap();
            };
            let mut progress_display = CallRateLimiter::new(0.1, progress_func);
            let scanned_paths_inner = scanned_paths.unwrap();
            let scanned_path_count = scanned_paths_inner.len();
            let mut scanned_paths_filtered = HashedAbsolutePathSet::default();
            for (index, file) in scanned_paths_inner.iter().enumerate() {
                if file.is_file() { scanned_paths_filtered.insert(file.clone()); }
                progress_display.call((index, scanned_path_count));
            }
            progress_display.call_unconditional((scanned_path_count, scanned_path_count));
            scanned_paths = Some(scanned_paths_filtered);
        }

        let mut clone_groups = file.clone_groups()?;
        if prune {
            let progress_func = |(index, total): (usize, usize)| {
                let percent = index * 100 / total;
                bunt::eprint!("\r{$green}INFO{/$}  {$bold}>{/$} Pruning clone files list {} / {} ({}%)", index.to_formatted_string(&Locale::en), total.to_formatted_string(&Locale::en), percent);
                io::stderr().flush().unwrap();
            };
            let mut progress_display = CallRateLimiter::new(0.1, progress_func);
            let file_count: usize = clone_groups.iter().map(|(_, group)| group.len()).sum();
            let mut clone_groups_filtered = vec![];
            let mut index = 0;
            for (size, group) in clone_groups {
                let mut group_filtered = vec![];
                for file in group {
                    if file.is_file() { group_filtered.push(file) }
                    progress_display.call((index, file_count));
                    index += 1;
                }
                if group_filtered.len() > 1 {
                    clone_groups_filtered.push((size, group_filtered));
                }
            }
            progress_display.call_unconditional((file_count, file_count));
            eprintln!();
            clone_groups = clone_groups_filtered;
        }

        Ok(Self { scanned_paths, clone_groups: CloneGroups::from(clone_groups) })
    }

    // /// returns an error if the specified path is not part of the directories scanned during the clones database's construction
    // fn check_part_of_scanned_paths<P: AsRef<Path> + std::fmt::Display>(&self, path: P) -> anyhow::Result<()> {
    //     if ! self.scanned_paths.contains_parent_of(&path) {
    //         return Err(anyhow!("`{path}` is not a child of the scanned paths"))
    //     }
    //     Ok(())
    // }

}