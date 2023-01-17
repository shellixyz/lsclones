
use std::{path::Path, collections::{HashMap, hash_map, btree_set}, fmt::Display, hash::Hash};

use derive_more::{Deref, IntoIterator, DerefMut};
use getset::{Getters, CopyGetters};
use anyhow::anyhow;
use itertools::Itertools;
use size::Size;

use crate::{clones::db::{ClonesDB, CloneRefGroup}, path::{HashedAbsolutePathRef, HashedAbsolutePath}, paths::{PathRefs, Paths, PathSet, PathRefSet}, fs::{dir, tree::TraversalOrder}};

use super::{FSTree, FilesIterKind, UpgradedNode, DirectoryNode, FilesIter};

impl FSTree {

    pub fn unique_files_iter<'a>(&'a self, clones_db: &'a ClonesDB) -> UniqueFilesIter<'a> {
        UniqueFilesIter { files_iter: self.traverse_files(TraversalOrder::Pre), clones_db }
    }

    // get the deepest path inside a clone dir which is also a clone dir because it meets the following conditions:
    // - the dir only contains no files (direct children)
    // - the dir only contains one dir
    fn clone_dir_deep_path<'a>(&'a self, clone_dir_node: &DirectoryNode) -> Option<&'a Path> {
        let mut clone_dir_node = self.0.get(clone_dir_node.node_id()).unwrap();
        let mut first = true;
        loop {
            if clone_dir_node.children().len() < 2 {
                let clone_dir_child_node_id = clone_dir_node.children().first().expect("a clone dir should not to be empty");
                let clone_dir_child_node = self.0.get(clone_dir_child_node_id).unwrap();
                if clone_dir_child_node.children().is_empty() {
                    break;
                } else {
                    clone_dir_node = clone_dir_child_node;
                }
            } else {
                break;
            }

            first = false;
        }

        if first {
            return None;
        }

        Some(&clone_dir_node.data().path)
    }

    pub fn clone_dirs<'a>(&'a self, dir: impl AsRef<Path>, clones_db: &'a ClonesDB, recursive: bool) -> anyhow::Result<Vec<CloneDir<'a>>> {
        let node = self.node_with_path(&dir)?;
        let UpgradedNode::DirectoryNode(dir_node) = node.upgrade() else { return Err(anyhow!("not a directory: {}", dir.as_ref().to_string_lossy())); };
        let files_iter_kind = if recursive { FilesIterKind::RecursivePreOrder } else { FilesIterKind::Children };
        let mut dirs_to_process = vec![dir_node.clone()];
        let mut clone_dirs = vec![];
        while let Some(current_dir) = dirs_to_process.pop() {
            let file_clones = current_dir.file_nodes_iter(files_iter_kind).map(|file_node| {
                clones_db.clone_group(file_node.path()).and_then(|group| {
                    let clones = group.filter_out_dir(current_dir.path());
                    if clones.is_empty() { None } else { Some((file_node.path(), clones)) }
                })
            }).collect::<Option<HashMap<_, _>>>();

            if let Some(file_clones) = file_clones {
                if ! file_clones.is_empty() {
                    let clone_dir = CloneDir {
                        path: current_dir.path(),
                        deep_path: self.clone_dir_deep_path(&current_dir),
                        clones: file_clones,
                        clones_db
                    };
                    clone_dirs.push(clone_dir);
                }
            } else if recursive {
                dirs_to_process.extend(current_dir.child_directory_nodes_iter());
            }
        }
        Ok(clone_dirs)
    }

    // returns files in dir without inside clones aka if the directory was isolated there would be no clones
    pub fn dir_decloned_inside_files(&self, dir: impl AsRef<Path>, recursive: bool, clones_db: &ClonesDB) -> anyhow::Result<PathRefSet> {
        let mut present_groups: Vec<&CloneRefGroup> = vec![];
        let files_iter_kind = if recursive { FilesIterKind::RecursivePreOrder } else { FilesIterKind::Children };
        let files =
            self
            .path_files_iter(dir, files_iter_kind)?
            .filter_map(|file| {
                let file = HashedAbsolutePathRef::new(file).expect("path should be absolute");
                if present_groups.iter().any(|group| group.contains(&file)) {
                    None
                } else {
                    if let Some(file_group) = clones_db.clone_group(&file) {
                        present_groups.push(file_group);
                    }
                    Some(file.inner())
                }
            })
            .collect();
        Ok(files)
    }

    // returns groups of identical clone dirs
    pub fn clone_dir_groups<'a>(&'a self, dir: impl AsRef<Path>, clones_db: &'a ClonesDB, recursive: bool) -> anyhow::Result<Vec<CloneDirGroup<'a>>> {
        let clone_dirs = self.clone_dirs(dir, clones_db, recursive)?;
        let mut selected = HashMap::new();
        let clone_dir_groups =
            clone_dirs
            .iter()
            .filter_map(|clone_dir| {

                if selected.contains_key(clone_dir.path) { return None; }

                let clone_dir_group = clone_dirs.iter().filter(|i_clone_dir| {

                    if selected.contains_key(i_clone_dir.path) { return false; }

                    if std::ptr::eq(clone_dir, *i_clone_dir) { return true; }

                    let i_clone_dir_path = HashedAbsolutePath::from(i_clone_dir.path.clone());
                    let has_missing_files = clone_dir.files_iter().any(|file|
                        match clones_db.clone_group(file) {
                            Some(group) =>
                                ! group.iter().any(|group_file| group_file.starts_with_hashed_path(&i_clone_dir_path)),
                            None => true
                        }
                    );

                    if has_missing_files {
                        false
                    } else {
                        let clone_dir_files = clone_dir.clones.iter().map(|(_, crg)|
                            crg.iter().map(|path| path.inner().to_owned()
                        )).flatten().collect::<PathSet>();
                        let i_clone_dir_files = dir::files_rec(i_clone_dir.path).into_set();
                        let has_extra_files = i_clone_dir_files.difference(&clone_dir_files).next().is_some();
                        !has_extra_files
                    }
                }).cloned().collect_vec();

                for clone_dir in &clone_dir_group {
                    selected.insert(clone_dir.path, ());
                }

                (!clone_dir_group.is_empty()).then_some(CloneDirGroup(clone_dir_group))

            }).collect_vec();
        Ok(clone_dir_groups)
    }

}

#[derive(Debug, Getters, CopyGetters, Clone)]
pub struct CloneDir<'a> {
    // #[getset(skip)]
    // scanned_dir_files: &'a DirFilesDB,
    clones_db: &'a ClonesDB,
    #[getset(get_copy = "pub")]
    path: &'a Path,
    #[getset(get = "pub")]
    deep_path: Option<&'a Path>,
    #[getset(get = "pub")]
    clones: HashMap<&'a Path, CloneRefGroup<'a>>
}

impl<'a> PartialEq for CloneDir<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl<'a> Eq for CloneDir<'a> {}

impl<'a> Hash for CloneDir<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

impl<'a> Display for CloneDir<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path.to_string_lossy())
    }
}

impl<'a> CloneDir<'a> {

    pub fn ref_dirs_iter(&self) -> RefDirsIter {
        let mut clones_iter = self.clones.iter();
        let first_clone_files_iter = clones_iter.next().unwrap().1.iter();
        RefDirsIter::new(self, clones_iter, first_clone_files_iter)
    }

    pub fn files_iter(&self) -> CloneDirFilesIter {
        CloneDirFilesIter(self.clones.keys())
    }

    pub fn file_count(&self) -> usize { self.clones.len() }

    pub fn size_bytes(&self) -> u64 {
        self.clones.iter().map(|(_, group)| group.file_size()).sum()
    }

    pub fn deep_path_rel(&self) -> Option<&'a Path> {
        Some(self.deep_path?.strip_prefix(self.path).unwrap())
    }

}

#[derive(Debug, Deref, DerefMut, IntoIterator)]
#[into_iterator(owned, ref)]
pub struct CloneDirs<'a>(Vec<CloneDir<'a>>);

impl<'a> CloneDirs<'a> {

    pub fn file_count(&self) -> usize {
        self.iter().map(CloneDir::file_count).sum()
    }

    pub fn size_bytes(&self) -> u64 {
        self.iter().map(CloneDir::size_bytes).sum()
    }

    pub fn size_human(&self) -> Size {
        Size::from_bytes(self.size_bytes())
    }

}

impl<'a> FromIterator<CloneDir<'a>> for CloneDirs<'a> {
    fn from_iter<T: IntoIterator<Item = CloneDir<'a>>>(iter: T) -> Self {
        CloneDirs(Vec::from_iter(iter.into_iter()))
    }
}

#[derive(Debug, Deref, IntoIterator)]
#[into_iterator(owned, ref)]
pub struct CloneDirGroup<'a>(Vec<CloneDir<'a>>);

impl<'a> CloneDirGroup<'a> {

    pub fn ref_dirs(&self) -> Vec<RefDir> {
        if self.0.is_empty() { return vec![]; }
        let first_clone_dir = self.0.first().unwrap();
        first_clone_dir.clones.iter().flat_map(|(_, clone_group)|
            clone_group.files().iter().filter_map(|file| {
                let part_of_clone_dir_group = self.0.iter().any(|cd| file.inner().starts_with(cd.path));
                (!part_of_clone_dir_group).then(|| file.parent().unwrap().to_path_buf())
            })
        )
        .unique()
        .map(|path| RefDir::new(first_clone_dir, HashedAbsolutePath::from(path)))
        .collect()
    }

    pub fn file_count(&self) -> usize {
        self.iter().map(CloneDir::file_count).sum()
    }

    pub fn size_bytes(&self) -> u64 {
        self.iter().map(CloneDir::size_bytes).sum()
    }

    // minimum as if there is no clone from these directories outside of them
    pub fn minimum_reclaimable_size(&self) -> u64 {
        if self.0.len() < 1 { return 0; }
        (self.0.len() - 1) as u64 * self.first().unwrap().size_bytes()
    }

}

#[derive(Debug, Deref, IntoIterator)]
pub struct CloneDirGroups<'a>(Vec<CloneDirGroup<'a>>);

impl<'a> CloneDirGroups<'a> {

    pub fn dir_count(&self) -> usize {
        self.iter().map(|group| group.len()).sum()
    }

    pub fn file_count(&self) -> usize {
        self.iter().map(CloneDirGroup::file_count).sum()
    }

    pub fn size_bytes(&self) -> u64 {
        self.iter().map(CloneDirGroup::size_bytes).sum()
    }

    pub fn size_human(&self) -> Size {
        Size::from_bytes(self.size_bytes())
    }

    pub fn minimum_reclaimable_size(&self) -> u64 {
        self.iter().map(CloneDirGroup::minimum_reclaimable_size).sum()
    }

    pub fn minimum_reclaimable_size_human(&self) -> Size {
        Size::from_bytes(self.minimum_reclaimable_size())
    }

}

impl<'a> FromIterator<CloneDirGroup<'a>> for CloneDirGroups<'a> {
    fn from_iter<T: IntoIterator<Item = CloneDirGroup<'a>>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

pub struct RefDirsIter<'a> {
    clone_dir: &'a CloneDir<'a>,
    clones_iter: hash_map::Iter<'a, &'a Path, CloneRefGroup<'a>>,
    clone_files_iter: btree_set::Iter<'a, HashedAbsolutePathRef<'a>>,
    returned: HashMap<&'a Path, ()>,
}

impl<'a> RefDirsIter<'a> {
    pub fn new(
        clone_dir: &'a CloneDir<'a>,
        clones_iter: hash_map::Iter<'a, &'a Path, CloneRefGroup<'a>>,
        clone_files_iter: btree_set::Iter<'a, HashedAbsolutePathRef<'a>>
    ) -> Self {
        Self { clone_dir, clones_iter, clone_files_iter, returned: HashMap::new() }
    }

}

impl<'a> Iterator for RefDirsIter<'a> {
    type Item = RefDir<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(clone_file) = self.clone_files_iter.next() {
                // file remains in clone_files_iter, get the parent directory
                let clone_file_dir = clone_file.parent().unwrap();
                // if this path has not been returned yet, insert it in the `returned` HashMap and return it
                if let hash_map::Entry::Vacant(entry) = self.returned.entry(clone_file_dir) {
                    entry.insert(());
                    let clone_file_dir = HashedAbsolutePath::from(clone_file_dir);
                    return Some(RefDir::new(self.clone_dir, clone_file_dir));
                }
                // this path has been returned already, start over with the next file in clone_files_iter
                continue;
            }

            // clone_files_iter has been fully consumed, replace it with the iterator over the next clone group if there is one
            if let Some((_file, clones)) = self.clones_iter.next() {
                self.clone_files_iter = clones.iter();
            } else {
                return None;
            }
        }
    }
}

#[derive(Debug, Clone, Deref, Getters)]
pub struct RefDir<'a> {
    clone_dir: &'a CloneDir<'a>,
    #[deref]
    #[getset(get = "pub")]
    path: HashedAbsolutePath,
}

impl<'a> Display for RefDir<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path.to_string_lossy())
    }
}

impl<'a> RefDir<'a> {
    pub fn new(clone_dir: &'a CloneDir<'a>, path: HashedAbsolutePath) -> Self { Self { clone_dir, path } }

    /// returns the list of files in the clone dir which match files in this RefDir
    pub fn clone_dir_clone_files(&self) -> PathRefs {
        self.clone_dir.clones.iter().filter_map(|(file, clones)|
            clones.iter().any(|clone| clone.parent_is_hap(&self.path)).then_some(*file)
        ).collect::<PathRefs>()
    }

    /// returns files which are in the clone dir but not in this RefDir
    pub fn missing(&self) -> PathRefs {
        self.clone_dir.files_iter().filter(|file|
            match self.clone_dir.clones_db.clone_group(file) {
                Some(group) =>
                    ! group.iter().any(|group_file| group_file.starts_with_hashed_path(&self.path)),
                None => true
            }
        ).collect()
    }

    pub fn clone_files(&self) -> PathRefs {
        self.clone_dir.clones
            .values()
            .flat_map(CloneRefGroup::files)
            .filter_map(|clone|
                clone.parent_is_hap(&self.path).then_some(AsRef::<Path>::as_ref(clone))
            )
            .collect()
    }

    /// returns files which are in this RefDir but not in the clone dir
    pub fn extra(&self) -> Paths {
        // FIXME: better than above but we might still be walking the same tree multiple times
        dir::files_rec(self.path.as_path()).into_set()
            .difference(&Paths::from(self.clone_files()).into_set())
            // don't count the file as extra if the file is in the clone dir which can happen if the clone dir is inside the ref dir
            .filter(|file| !file.starts_with(self.clone_dir.path))
            .cloned().collect()
    }

}

pub struct CloneDirFilesIter<'a>(hash_map::Keys<'a, &'a Path, CloneRefGroup<'a>>);

impl<'a> Iterator for CloneDirFilesIter<'a> {
    type Item = &'a Path;

    fn next(&mut self) -> Option<Self::Item> {
        Some(*self.0.next()?)
    }
}

pub struct UniqueFilesIter<'a> {
    files_iter: FilesIter<'a>,
    clones_db: &'a ClonesDB,
}

impl<'a> Iterator for UniqueFilesIter<'a> {
    type Item = &'a Path;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let file = self.files_iter.next()?;
            if ! self.clones_db.file_is_a_clone(file) {
                return Some(file);
            }
        }
    }
}