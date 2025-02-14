
use std::{fs, os::unix::prelude::MetadataExt, path::{PathBuf, Path}, collections::VecDeque};

use anyhow::anyhow;

use crate::{clones::db::ClonesDB, paths::Paths, error_behavior::ErrorBehavior};

use super::Tree;


// pub fn files<P: AsRef<StdPath>>(path: P) -> anyhow::Result<Paths> {
//     let entry_iter = fs::read_dir(&path).map_err(|error|
//         anyhow!("failed to read directory `{}`: {error}", path.as_ref().to_string_lossy())
//     )?;
//     let mut files = Paths::default();
//     for entry in entry_iter {
//         let entry_path = entry.map_err(|error|
//             anyhow!("failed reading entry in directory `{}`: {error}", path.as_ref().to_string_lossy())
//         )?.path();
//         if entry_path.is_file() {
//             files.push(PathBuf::from(entry_path.to_str().unwrap()))
//         }
//     }
//     Ok(files)
// }

pub struct DirWalker {
    dirs_to_process: VecDeque<PathBuf>,
    current_dir_entries: Vec<PathBuf>,
    error_behavior: ErrorBehavior,
}

impl DirWalker {
    pub fn new(dir: impl Into<PathBuf>, error_behavior: ErrorBehavior) -> Self {
        Self { dirs_to_process: VecDeque::from_iter(Some(dir.into())), current_dir_entries: vec![], error_behavior }
    }
}

impl Iterator for DirWalker {
    type Item = anyhow::Result<PathBuf>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(entry) = self.current_dir_entries.pop() {
                return Some(Ok(entry));
            }

            use ErrorBehavior::*;
            if let Some(dir) = self.dirs_to_process.pop_back() {
                let dir_iter = match std::fs::read_dir(&dir) {
                    Ok(dir_iter) => dir_iter,
                    Err(e) => match self.error_behavior {
                        Ignore => continue,
                        Display | Stop => {
                            let error_string = format!("failed reading `{}`: {e}", dir.to_string_lossy());
                            match self.error_behavior {
                                Display => {
                                    eprintln!("{error_string}");
                                    continue;
                                },
                                Stop => return Some(Err(anyhow!("{error_string}"))),
                                _ => unreachable!()
                            }
                        },
                    }
                };
                for entry in dir_iter {
                    match entry {
                        Ok(entry) => {
                            match entry.file_type() {
                                Ok(file_type) =>
                                    if file_type.is_file() {
                                        self.current_dir_entries.push(entry.path());
                                    } else if file_type.is_dir() {
                                        self.dirs_to_process.push_front(entry.path());
                                    },
                                Err(e) => match self.error_behavior {
                                    Ignore => continue,
                                    Display | Stop => {
                                        let error_string = format!("failed to get file type of `{}`: `{e}", entry.path().to_string_lossy());
                                        match self.error_behavior {
                                            Display => {
                                                eprintln!("{error_string}");
                                                continue;
                                            },
                                            Stop => return Some(Err(anyhow!("{error_string}"))),
                                            _ => unreachable!()
                                        }
                                    },
                                }
                            }
                        },
                        Err(e) => return Some(Err(anyhow!("{e}")))
                    }
                }
            } else {
                return None;
            }
        }
    }
}

pub fn dirs<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<PathBuf>> {
    let entry_iter = fs::read_dir(&path).map_err(|error|
        anyhow!("failed to read directory `{}`: {error}", path.as_ref().to_string_lossy())
    )?;
    let mut files = vec![];
    for entry in entry_iter {
        let entry_path = entry.map_err(|error|
            anyhow!("failed reading entry in directory `{}`: {error}", path.as_ref().to_string_lossy())
        )?.path();
        if entry_path.is_dir() {
            files.push(entry_path);
        }
    }
    Ok(files)
}

pub struct FilesWalker(walkdir::IntoIter);

impl Iterator for FilesWalker {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.0.next()? {
                Ok(entry) => {
                    let entry_path = entry.path();
                    if entry_path.is_file() {
                        return Some(entry_path.to_path_buf())
                    }
                },
                Err(error) => {
                    if let Some(path) = error.path() {
                        let error_str = match error.io_error() {
                            Some(error) => error.to_string(),
                            None => "unknown".to_owned(),
                        };
                        eprintln!("error listing files from `{}`: {}", path.to_string_lossy(), error_str);
                    }
                },
            }
        }
    }
}

pub fn walk_files<P: AsRef<Path>>(path: P) -> FilesWalker {
    FilesWalker(walkdir::WalkDir::new(path).into_iter())
}

pub fn files_rec<P: AsRef<Path>>(path: P) -> Paths {
    walk_files(path).collect()
}

pub struct DirsWalker {
    first: bool,
    iter: walkdir::IntoIter
}

impl Iterator for DirsWalker {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next()? {
                Ok(entry) => {
                    if self.first {
                        self.first = false;
                        continue;
                    }
                    let entry_path = entry.path();
                    if entry_path.is_dir() {
                        return Some(entry_path.to_path_buf());
                    }
                },
                Err(error) => {
                    if let Some(path) = error.path() {
                        let error_str = match error.io_error() {
                            Some(error) => error.to_string(),
                            None => "unknown".to_owned(),
                        };
                        eprintln!("error listing files from `{}`: {}", path.to_string_lossy(), error_str);
                    }
                },
            }
        }
    }
}

pub fn walk_dirs<P: AsRef<Path>>(path: P) -> DirsWalker {
    DirsWalker { first: true, iter: walkdir::WalkDir::new(path).into_iter() }
}

// pub fn dirs_rec<P: AsRef<Path>>(path: P) -> Paths {
//     walk_dirs(path).collect()
// }

/// returns true if the specified directory only contains uniq files
pub fn is_unique_dir<P: AsRef<Path>>(dir: P, clones_db: &ClonesDB) -> bool {
    clones_db.dir_clone_files_iter(dir, true).next().is_none()
}

// /// returns dirs which only contain uniq files
pub fn unique_dirs<P: Into<PathBuf>>(dir: P, recursive: bool, clones_db: &ClonesDB) -> anyhow::Result<Vec<PathBuf>> {
    let dir = dir.into();
    if recursive {
        let mut dirs_to_process = vec![dir];
        let mut unique_dirs = vec![];
        while let Some(current_dir) = dirs_to_process.pop() {
            if is_unique_dir(&current_dir, clones_db) {
                unique_dirs.push(current_dir);
            } else {
                dirs_to_process.extend(dirs(current_dir)?.into_iter());
            }
        }
        Ok(unique_dirs)
    } else {
        Ok(dirs(dir)?.into_iter().filter(|idir| is_unique_dir(idir, clones_db)).collect())
    }
}

/// returns directory size in bytes
pub fn size<P: AsRef<Path>>(dir: P) -> u64 {
    walk_files(dir).map(|file| std::fs::metadata(file).unwrap().size()).sum()
}

pub fn file_tree(dir: impl AsRef<Path>, error_behavior: ErrorBehavior) -> anyhow::Result<Tree> {
    let mut tree = Tree::default();
    tree.extend_with_progress(dir, error_behavior, |_, _|{})?;
    Ok(tree)
}