use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use path_absolutize::Absolutize;

use crate::path::{HashedAbsolutePath, HashedAbsolutePathSet};

pub struct File {
    path: PathBuf,
    content: HashMap<String, serde_json::Value>,
}

impl File {
    /// reads clones database file in json format
    pub fn open<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let file = fs_err::File::open(&path)?;
        let file_buf = io::BufReader::new(file);
        let content: HashMap<String, serde_json::Value> = serde_json::from_reader(file_buf)
            .map_err(|err| {
                anyhow!(
                    "failed opening clones file {}: {err}",
                    path.as_ref().to_string_lossy()
                )
            })?;
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            content,
        })
    }

    fn missing_item(&self, section: &str) -> anyhow::Error {
        anyhow!(
            "could not find {section} section in dups file: {}",
            self.path.to_string_lossy()
        )
    }

    pub fn scanned_paths(&self) -> anyhow::Result<Option<HashedAbsolutePathSet>> {
        let Some(serde_json::Value::Object(json_header)) = self.content.get("header") else {
            return Err(self.missing_item("header"));
        };

        let Some(serde_json::Value::Array(json_paths)) = json_header.get("paths") else {
            return Ok(None);
        };

        let base_dir =
            if let Some(serde_json::Value::String(base_dir)) = json_header.get("base_dir") {
                Path::new(base_dir)
            } else {
                return Err(self.missing_item("header/base_dir"));
            };

        let mut scanned_paths = HashedAbsolutePathSet::new();

        for path_json_value in json_paths {
            match path_json_value {
                serde_json::Value::String(path_str) => {
                    let dir_path = Path::new(path_str)
                        .absolutize_from(base_dir)
                        .unwrap()
                        .to_path_buf();
                    scanned_paths.insert(HashedAbsolutePath::from(dir_path));
                }
                _ => return Err(anyhow!("bad value type in header/paths")),
            }
        }

        Ok(Some(scanned_paths))
    }

    pub fn clone_groups(&self) -> anyhow::Result<Vec<(u64, Vec<HashedAbsolutePath>)>> {
        let Some(serde_json::Value::Array(json_clone_groups)) = self.content.get("groups") else {
            return Err(self.missing_item("groups"));
        };

        let mut clone_groups = Vec::with_capacity(json_clone_groups.len());

        for json_clone_group in json_clone_groups {
            match json_clone_group {
                serde_json::Value::Object(json_clone_group) => {
                    let file_len = if let Some(serde_json::Value::Number(file_len)) =
                        json_clone_group.get("file_len")
                    {
                        file_len
                            .as_u64()
                            .ok_or_else(|| anyhow!("bad group/file_len value: {file_len:?}"))?
                    } else {
                        return Err(self.missing_item("group/file_len"));
                    };

                    let Some(serde_json::Value::Array(files)) = json_clone_group.get("files")
                    else {
                        return Err(self.missing_item("group/files"));
                    };

                    let mut group_files = Vec::with_capacity(files.len());
                    for file in files {
                        match file {
                            serde_json::Value::String(path_str) => {
                                group_files.push(HashedAbsolutePath::from(path_str))
                            }
                            _ => return Err(anyhow!("bad value type in clone file group")),
                        }
                    }
                    if group_files.len() < 2 {
                        log::warn!(
                            "found group with less than 2 files in {}",
                            self.path.to_string_lossy()
                        );
                    }
                    clone_groups.push((file_len, group_files))
                }
                _ => return Err(anyhow!("bad value type in groups: {json_clone_group:?}")),
            }
        }

        Ok(clone_groups)
    }
}
