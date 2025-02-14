use std::{
    borrow::{Borrow, Cow},
    collections::VecDeque,
    error::Error,
    ffi::{OsStr, OsString},
    fmt::Display,
    hash::Hash,
    path::{self, Path, PathBuf},
};

use anyhow::anyhow;
use derive_more::IsVariant;
use id_tree::{
    self, InsertBehavior, LevelOrderTraversalIds, Node as IDTreeNode, PreOrderTraversalIds,
};
pub use id_tree::{NodeId, NodeIdError};
use itertools::Itertools;
use path_absolutize::Absolutize;
use tap::Tap;

use crate::error_behavior::ErrorBehavior;

pub mod clones;

#[derive(Debug, thiserror::Error)]
#[error("path `{path}` is not part of node `{node_path}`")]
pub struct PathNotPartOfNode {
    path: PathBuf,
    node_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum NodePathNodeIdError {
    #[error(transparent)]
    PathNotPartOfNode(#[from] PathNotPartOfNode),
    #[error(transparent)]
    PathNotFound(#[from] PathNotFound),
}

#[derive(Debug, Clone, Copy, IsVariant)]
pub enum PathKind {
    File,
    Directory,
}

impl PathKind {
    fn node_data(&self, path: impl Into<PathBuf>) -> NodeData {
        match self {
            PathKind::File => NodeData::file(path),
            PathKind::Directory => NodeData::directory(path),
        }
    }
}

#[derive(Debug, Clone, Copy, IsVariant, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy)]
pub enum FilesIterKind {
    Children,
    RecursivePreOrder,
    RecursiveLevelOrder,
}

#[derive(Debug, Clone, Copy)]
enum InternalFilesIterKind {
    Children,
    Recursive(TraversalOrder),
}

impl From<FilesIterKind> for InternalFilesIterKind {
    fn from(files_iter_kind: FilesIterKind) -> Self {
        use FilesIterKind::*;
        match files_iter_kind {
            Children => Self::Children,
            RecursivePreOrder => Self::Recursive(TraversalOrder::Pre),
            RecursiveLevelOrder => Self::Recursive(TraversalOrder::Level),
        }
    }
}

#[derive(Debug, Clone)]
struct NodeData {
    kind: NodeKind,
    name: OsString,
    path: PathBuf,
}

impl NodeData {
    fn new(path: impl Into<PathBuf>, kind: NodeKind) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .map(OsStr::to_os_string)
            .unwrap_or_else(|| "/".into());
        Self { kind, name, path }
    }

    fn directory(path: impl Into<PathBuf>) -> Self {
        Self::new(path, NodeKind::Directory)
    }

    fn file(path: impl Into<PathBuf>) -> Self {
        Self::new(path, NodeKind::File)
    }

    fn is_directory(&self) -> bool {
        self.kind.is_directory()
    }
    fn is_file(&self) -> bool {
        self.kind.is_file()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("node is not a directory: {0}")]
pub struct NodeIsNotADirectory(PathBuf);

#[derive(Debug, thiserror::Error)]
#[error("path is not a directory: {0}")]
pub struct PathIsNotADirectory(PathBuf);

#[derive(Debug, Clone, IsVariant)]
pub enum UpgradedNode<'a> {
    DirectoryNode(DirectoryNode<'a>),
    FileNode(FileNode<'a>),
}

#[derive(Debug, Clone)]
pub struct Node<'a> {
    tree: &'a FSTree,
    node_id: Cow<'a, NodeId>,
    node: &'a IDTreeNode<NodeData>,
}

impl<'a> Node<'a> {
    fn new(tree: &'a FSTree, node_id: Cow<'a, NodeId>, node: &'a IDTreeNode<NodeData>) -> Self {
        Self {
            tree,
            node_id,
            node,
        }
    }

    fn absolutize_path_impl<'b>(
        node_path: &Path,
        path: &'b Path,
    ) -> Result<Cow<'b, Path>, PathNotPartOfNode> {
        if path.is_absolute() {
            if !path.starts_with(node_path) {
                return Err(PathNotPartOfNode {
                    path: path.to_owned(),
                    node_path: node_path.to_owned(),
                });
            }
            return Ok(Cow::Borrowed(path));
        }
        Ok(Cow::Owned(node_path.join(path)))
    }

    fn path_node_id_impl(
        tree: &'a FSTree,
        node_path: &Path,
        path: &Path,
    ) -> Result<&'a NodeId, NodePathNodeIdError> {
        Ok(tree.path_node_id(Self::absolutize_path_impl(node_path, path)?)?)
    }

    pub fn name(&self) -> &'a OsStr {
        self.node.data().name.as_os_str()
    }
    pub fn path(&self) -> &'a Path {
        self.node.data().path.as_path()
    }
    pub fn kind(&self) -> NodeKind {
        self.node.data().kind
    }
    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }
    pub fn is_directory(&self) -> bool {
        self.node.data().is_directory()
    }
    pub fn is_file(&self) -> bool {
        self.node.data().is_file()
    }

    pub fn parent(&self) -> Option<Node> {
        Some(self.tree.node_with_id(self.node.parent()?).unwrap())
    }

    fn check_is_directory(&self) -> Result<(), NodeIsNotADirectory> {
        if !self.node.data().is_directory() {
            return Err(NodeIsNotADirectory(self.node.data().path.clone()));
        }
        Ok(())
    }

    /// returns Ok(true) if the node is a directory and does not contain any file
    /// returns Err(NodeIsNotADirectory) if the node is not a directory
    pub fn is_empty(&self) -> Result<bool, NodeIsNotADirectory> {
        Ok(self
            .files_iter(FilesIterKind::RecursivePreOrder)?
            .next()
            .is_none())
    }

    pub fn files_iter(&self, kind: FilesIterKind) -> Result<FilesIter<'a>, NodeIsNotADirectory> {
        use InternalFilesIterKind::*;
        match InternalFilesIterKind::from(kind) {
            Children => self.child_files_iter(),
            Recursive(order) => self.traverse_files(order),
        }
    }

    pub fn file_nodes_iter(
        &self,
        kind: FilesIterKind,
    ) -> Result<FileNodesIter<'a>, NodeIsNotADirectory> {
        use InternalFilesIterKind::*;
        match InternalFilesIterKind::from(kind) {
            Children => self.child_file_nodes_iter(),
            Recursive(order) => self.traverse_file_nodes(order),
        }
    }

    pub fn children_count(&self) -> Result<usize, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(self.node.children().len())
    }

    pub fn child_nodes_iter(&self) -> Result<NodesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(NodesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
            None,
        ))
    }

    pub fn children_kind_iter(&self, kind: NodeKind) -> Result<NodesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(NodesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
            Some(kind),
        ))
    }

    pub fn child_directories_iter(&self) -> Result<DirectoriesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(DirectoriesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
        ))
    }

    pub fn child_directory_nodes_iter(
        &self,
    ) -> Result<DirectoryNodesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(DirectoryNodesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
        ))
    }

    pub fn child_files_iter(&self) -> Result<FilesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(FilesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
        ))
    }

    pub fn child_file_nodes_iter(&self) -> Result<FileNodesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(FileNodesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
        ))
    }

    // fn absolutize_path<'b>(&self, path: &'b Path) -> Result<Cow<'b, Path>, PathNotPartOfNode> {
    //     Node::absolutize_path_impl(self.path(), path.as_ref())
    // }

    pub fn path_node_id(&self, path: &Path) -> Result<&'a NodeId, NodePathNodeIdError> {
        Node::path_node_id_impl(self.tree, self.path(), path)
    }

    pub fn traverse_nodes(
        &self,
        order: TraversalOrder,
        include_self: bool,
    ) -> Result<NodesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(NodesIter::traverse(self.tree, &self.node_id, include_self, None, order).unwrap())
    }

    pub fn traverse_path_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<NodesIter<'a>, TraversePathError> {
        self.check_is_directory()?;
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(NodesIter::traverse(self.tree, node_id, include_path, None, order).unwrap())
    }

    pub fn traverse_directory_nodes(
        &self,
        order: TraversalOrder,
        include_self: bool,
    ) -> Result<DirectoryNodesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(DirectoryNodesIter::traverse(self.tree, &self.node_id, include_self, order).unwrap())
    }

    pub fn traverse_directories(
        &self,
        order: TraversalOrder,
        include_self: bool,
    ) -> Result<DirectoriesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(DirectoriesIter::traverse(self.tree, &self.node_id, include_self, order).unwrap())
    }

    pub fn traverse_path_directory_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<DirectoryNodesIter<'a>, TraversePathError> {
        self.check_is_directory()?;
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(DirectoryNodesIter::traverse(self.tree, node_id, include_path, order).unwrap())
    }

    pub fn traverse_path_directories(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<DirectoriesIter<'a>, TraversePathError> {
        self.check_is_directory()?;
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(DirectoriesIter::traverse(self.tree, node_id, include_path, order).unwrap())
    }

    pub fn traverse_files(
        &self,
        order: TraversalOrder,
    ) -> Result<FilesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(FilesIter::traverse(self.tree, &self.node_id, true, order).unwrap())
    }

    pub fn traverse_file_nodes(
        &self,
        order: TraversalOrder,
    ) -> Result<FileNodesIter<'a>, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(FileNodesIter::traverse(self.tree, &self.node_id, true, order).unwrap())
    }

    pub fn traverse_path_files(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
    ) -> Result<FilesIter<'a>, TraversePathError> {
        self.check_is_directory()?;
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(FilesIter::traverse(self.tree, node_id, true, order).unwrap())
    }

    pub fn traverse_path_file_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
    ) -> Result<FileNodesIter<'a>, TraversePathError> {
        self.check_is_directory()?;
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(FileNodesIter::traverse(self.tree, node_id, true, order).unwrap())
    }

    pub fn node_count(&self) -> Result<usize, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(self.tree.node_count_base(&self.node_id))
    }

    pub fn path_node_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        self.check_is_directory()?;
        let node_id = self.path_node_id(path.as_ref())?;
        Ok(self.tree.node_count_base(node_id))
    }

    pub fn directory_count(&self) -> Result<usize, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(self
            .traverse_directories(TraversalOrder::Pre, true)?
            .count())
    }

    pub fn path_directory_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        self.check_is_directory()?;
        Ok(self
            .traverse_path_directories(path, TraversalOrder::Pre, true)?
            .count())
    }

    pub fn file_count(&self) -> Result<usize, NodeIsNotADirectory> {
        self.check_is_directory()?;
        Ok(self.traverse_files(TraversalOrder::Pre)?.count())
    }

    pub fn path_file_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        self.check_is_directory()?;
        Ok(self.traverse_path_files(path, TraversalOrder::Pre)?.count())
    }

    // XXX would need a refcell
    // pub fn insert_path(&mut self, path: impl Into<PathBuf>, kind: PathKind) -> Result<Node, InsertChildError> {
    //     self.check_is_directory()?;
    //     self.tree.insert_child(&self.node_id, path, kind)
    // }

    // pub fn insert_directory(&mut self, path: impl Into<PathBuf>) -> Result<DirectoryNode, InsertChildError> {
    //     self.check_is_directory()?;
    //     self.tree.insert_child_directory(&self.node_id, path)
    // }

    // pub fn insert_file(&mut self, path: impl Into<PathBuf>) -> Result<FileNode, InsertChildError> {
    //     self.check_is_directory()?;
    //     self.tree.insert_child_file(&self.node_id, path)
    // }

    pub fn upgrade(self) -> UpgradedNode<'a> {
        if self.is_directory() {
            let dir_node = DirectoryNode {
                tree: self.tree,
                node_id: self.node_id,
                node: self.node,
            };
            UpgradedNode::DirectoryNode(dir_node)
        } else if self.is_file() {
            let file_node = FileNode {
                tree: self.tree,
                node_id: self.node_id,
                node: self.node,
            };
            UpgradedNode::FileNode(file_node)
        } else {
            unreachable!()
        }
    }

    pub fn upgrade_to_directory_node(self) -> DirectoryNode<'a> {
        if !self.is_directory() {
            panic!(
                "was not a directory node: {}",
                self.path().to_string_lossy()
            );
        }
        let UpgradedNode::DirectoryNode(dir_node) = self.upgrade() else {
            unreachable!()
        };
        dir_node
    }

    pub fn upgrade_to_file_node(self) -> FileNode<'a> {
        if !self.is_file() {
            panic!(
                "was not a directory node: {}",
                self.path().to_string_lossy()
            );
        }
        let UpgradedNode::FileNode(file_node) = self.upgrade() else {
            unreachable!()
        };
        file_node
    }
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.node_id == other.node_id
    }
}

impl Eq for Node<'_> {}

impl Hash for Node<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.node_id.hash(state);
    }
}

impl Display for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.node.data().path.to_string_lossy())
    }
}

#[derive(Debug, Clone)]
pub struct DirectoryNode<'a> {
    tree: &'a FSTree,
    node_id: Cow<'a, NodeId>,
    node: &'a IDTreeNode<NodeData>,
}

impl<'a> DirectoryNode<'a> {
    fn new(tree: &'a FSTree, node_id: Cow<'a, NodeId>, node: &'a IDTreeNode<NodeData>) -> Self {
        Self {
            tree,
            node_id,
            node,
        }
    }

    pub fn name(&self) -> &'a OsStr {
        self.node.data().name.as_os_str()
    }
    pub fn path(&self) -> &'a Path {
        self.node.data().path.as_path()
    }
    pub fn kind(&self) -> NodeKind {
        self.node.data().kind
    }
    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    pub fn parent(&self) -> Option<Node> {
        Some(self.tree.node_with_id(self.node.parent()?).unwrap())
    }

    /// returns true if the node does not contain any file
    pub fn is_empty(&self) -> bool {
        self.files_iter(FilesIterKind::RecursivePreOrder)
            .next()
            .is_none()
    }

    pub fn files_iter(&self, kind: FilesIterKind) -> FilesIter {
        use InternalFilesIterKind::*;
        match InternalFilesIterKind::from(kind) {
            Children => self.child_files_iter(),
            Recursive(order) => self.traverse_files(order),
        }
    }

    pub fn file_nodes_iter(&self, kind: FilesIterKind) -> FileNodesIter<'a> {
        use InternalFilesIterKind::*;
        match InternalFilesIterKind::from(kind) {
            Children => self.child_file_nodes_iter(),
            Recursive(order) => self.traverse_file_nodes(order),
        }
    }

    pub fn children_count(&self) -> usize {
        self.node.children().len()
    }

    pub fn child_nodes_iter(&self) -> NodesIter<'a> {
        NodesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
            None,
        )
    }

    pub fn children_kind_iter(&self, kind: NodeKind) -> NodesIter<'a> {
        NodesIter::children(
            self.tree,
            self.tree.0.children_ids(&self.node_id).unwrap(),
            Some(kind),
        )
    }

    pub fn child_directories_iter(&self) -> DirectoriesIter<'a> {
        DirectoriesIter::children(self.tree, self.tree.0.children_ids(&self.node_id).unwrap())
    }

    pub fn child_directory_nodes_iter(&self) -> DirectoryNodesIter<'a> {
        DirectoryNodesIter::children(self.tree, self.tree.0.children_ids(&self.node_id).unwrap())
    }

    pub fn child_files_iter(&self) -> FilesIter<'a> {
        FilesIter::children(self.tree, self.tree.0.children_ids(&self.node_id).unwrap())
    }

    pub fn child_file_nodes_iter(&self) -> FileNodesIter<'a> {
        FileNodesIter::children(self.tree, self.tree.0.children_ids(&self.node_id).unwrap())
    }

    // fn absolutize_path<'b>(&self, path: &'b Path) -> Result<Cow<'b, Path>, PathNotPartOfNode> {
    //     Node::absolutize_path_impl(self.path(), path.as_ref())
    // }

    pub fn path_node_id(&self, path: &Path) -> Result<&'a NodeId, NodePathNodeIdError> {
        Node::path_node_id_impl(self.tree, self.path(), path)
    }

    pub fn traverse_nodes(&self, order: TraversalOrder, include_self: bool) -> NodesIter<'a> {
        NodesIter::traverse(self.tree, self.node_id(), include_self, None, order).unwrap()
    }

    pub fn traverse_path_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<NodesIter<'a>, TraversePathError> {
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(NodesIter::traverse(self.tree, node_id, include_path, None, order).unwrap())
    }

    pub fn traverse_directory_nodes(
        &self,
        order: TraversalOrder,
        include_self: bool,
    ) -> DirectoryNodesIter<'a> {
        DirectoryNodesIter::traverse(self.tree, self.node_id(), include_self, order).unwrap()
    }

    pub fn traverse_directories(
        &self,
        order: TraversalOrder,
        include_self: bool,
    ) -> DirectoriesIter<'a> {
        DirectoriesIter::traverse(self.tree, self.node_id(), include_self, order).unwrap()
    }

    pub fn traverse_path_directory_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<DirectoryNodesIter<'a>, TraversePathError> {
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(DirectoryNodesIter::traverse(self.tree, node_id, include_path, order).unwrap())
    }

    pub fn traverse_path_directories(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<DirectoriesIter<'a>, TraversePathError> {
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(DirectoriesIter::traverse(self.tree, node_id, include_path, order).unwrap())
    }

    pub fn traverse_files(&self, order: TraversalOrder) -> FilesIter<'a> {
        FilesIter::traverse(self.tree, self.node_id(), true, order).unwrap()
    }

    pub fn traverse_file_nodes(&self, order: TraversalOrder) -> FileNodesIter<'a> {
        FileNodesIter::traverse(self.tree, self.node_id(), true, order).unwrap()
    }

    pub fn traverse_path_files(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
    ) -> Result<FilesIter<'a>, TraversePathError> {
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(FilesIter::traverse(self.tree, node_id, true, order).unwrap())
    }

    pub fn traverse_path_file_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
    ) -> Result<FileNodesIter<'a>, TraversePathError> {
        let node_id = self.path_node_id(path.as_ref())?;
        self.tree.check_node_is_directory(node_id)?;
        Ok(FileNodesIter::traverse(self.tree, node_id, true, order).unwrap())
    }

    pub fn node_count(&self) -> usize {
        self.tree.node_count_base(&self.node_id)
    }

    pub fn path_node_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        let node_id = self.path_node_id(path.as_ref())?;
        Ok(self.tree.node_count_base(node_id))
    }

    pub fn directories_count(&self) -> usize {
        self.traverse_directories(TraversalOrder::Pre, true).count()
    }

    pub fn path_directory_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        Ok(self
            .traverse_path_directories(path, TraversalOrder::Pre, true)?
            .count())
    }

    pub fn file_count(&self) -> usize {
        self.traverse_files(TraversalOrder::Pre).count()
    }

    pub fn path_file_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        Ok(self.traverse_path_files(path, TraversalOrder::Pre)?.count())
    }

    // XXX would require refcell
    // pub fn insert_path(&mut self, path: impl Into<PathBuf>, kind: PathKind) -> Result<Node, InsertChildError> {
    //     self.tree.insert_child(&self.node_id, path, kind)
    // }

    // pub fn insert_directory(&mut self, path: impl Into<PathBuf>) -> Result<DirectoryNode, InsertChildError> {
    //     self.tree.insert_child_directory(&self.node_id, path)
    // }

    // pub fn insert_file(&mut self, path: impl Into<PathBuf>) -> Result<FileNode, InsertChildError> {
    //     self.tree.insert_child_file(&self.node_id, path)
    // }

    pub fn downgrade(self) -> Node<'a> {
        Node {
            tree: self.tree,
            node_id: self.node_id,
            node: self.node,
        }
    }
}

impl PartialEq for DirectoryNode<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.node_id == other.node_id
    }
}

impl Eq for DirectoryNode<'_> {}

impl Hash for DirectoryNode<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.node_id.hash(state);
    }
}

impl Display for DirectoryNode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.node.data().path.to_string_lossy())
    }
}

#[derive(Debug, Clone)]
pub struct FileNode<'a> {
    tree: &'a FSTree,
    node_id: Cow<'a, NodeId>,
    node: &'a IDTreeNode<NodeData>,
}

impl<'a> FileNode<'a> {
    fn new(tree: &'a FSTree, node_id: Cow<'a, NodeId>, node: &'a IDTreeNode<NodeData>) -> Self {
        Self {
            tree,
            node_id,
            node,
        }
    }

    pub fn name(&self) -> &'a OsStr {
        self.node.data().name.as_os_str()
    }
    pub fn path(&self) -> &'a Path {
        self.node.data().path.as_path()
    }
    pub fn kind(&self) -> NodeKind {
        self.node.data().kind
    }
    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    pub fn parent(&self) -> Option<Node> {
        Some(self.tree.node_with_id(self.node.parent()?).unwrap())
    }
}

impl PartialEq for FileNode<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.node_id == other.node_id
    }
}

impl Eq for FileNode<'_> {}

impl Hash for FileNode<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.node_id.hash(state);
    }
}

impl Display for FileNode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.node.data().path.to_string_lossy())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InsertChildError {
    #[error(transparent)]
    NodeId(#[from] NodeIdError),
    #[error(transparent)]
    NodeIsNotADirectory(#[from] NodeIsNotADirectory),
    #[error("path `{path}` does not start with node path `{node_path}`")]
    Hierarchy { path: PathBuf, node_path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
#[error("path not found: {0}")]
pub struct PathNotFound(PathBuf);

#[derive(Debug, thiserror::Error)]
pub enum TraversePathError {
    #[error(transparent)]
    NodeIsNotADirectory(#[from] NodeIsNotADirectory),
    #[error(transparent)]
    PathNotFound(#[from] PathNotFound),
    #[error(transparent)]
    PathIsNotADirectory(#[from] PathIsNotADirectory),
    #[error(transparent)]
    PathNotPartOfNode(#[from] PathNotPartOfNode),
}

impl From<NodePathNodeIdError> for TraversePathError {
    fn from(node_path_node_id_error: NodePathNodeIdError) -> Self {
        match node_path_node_id_error {
            NodePathNodeIdError::PathNotPartOfNode(inner) => Self::PathNotPartOfNode(inner),
            NodePathNodeIdError::PathNotFound(inner) => Self::PathNotFound(inner),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FSTree(id_tree::Tree<NodeData>);

impl Default for FSTree {
    fn default() -> Self {
        Self(Default::default()).tap_mut(|tree| tree.insert_root_node())
    }
}

impl FSTree {
    pub fn root_node_id(&self) -> &NodeId {
        self.0.root_node_id().unwrap()
    }

    pub fn root_node(&self) -> Node {
        self.node_with_id(self.root_node_id()).unwrap()
    }

    fn insert_node(
        &mut self,
        node: NodeData,
        behavior: id_tree::InsertBehavior,
    ) -> Result<NodeId, NodeIdError> {
        self.0.insert(IDTreeNode::new(node), behavior)
    }

    fn insert_root_node(&mut self) {
        self.insert_node(NodeData::directory("/"), InsertBehavior::AsRoot)
            .unwrap();
    }

    /// insert child node without checking the node is a directory and not a file which cannot have children
    /// and not checking whether `path` is starts with the node path
    pub fn insert_child_unchecked_impl(
        &mut self,
        node_id: &NodeId,
        path: impl Into<PathBuf>,
        kind: PathKind,
    ) -> Result<NodeId, NodeIdError> {
        self.insert_node(kind.node_data(path), InsertBehavior::UnderNode(node_id))
    }

    pub fn insert_child_unchecked(
        &mut self,
        node_id: &NodeId,
        path: impl Into<PathBuf>,
        kind: PathKind,
    ) -> Result<Node, NodeIdError> {
        let node_id = self.insert_child_unchecked_impl(node_id, path, kind)?;
        let node = self.0.get(&node_id).unwrap();
        Ok(Node::new(self, Cow::Owned(node_id), node))
    }

    pub fn insert_child_impl(
        &mut self,
        node_id: &NodeId,
        path: impl Into<PathBuf>,
        kind: PathKind,
    ) -> Result<NodeId, InsertChildError> {
        let node_data = self.0.get(node_id)?.data();
        if node_data.is_file() {
            Err(NodeIsNotADirectory(node_data.path.clone()))?
        }
        let path = path.into();
        if !path.starts_with(&node_data.path) {
            return Err(InsertChildError::Hierarchy {
                path,
                node_path: node_data.path.to_owned(),
            });
        }
        Ok(self.insert_child_unchecked_impl(node_id, path, kind)?)
    }

    pub fn insert_child(
        &mut self,
        node_id: &NodeId,
        path: impl Into<PathBuf>,
        kind: PathKind,
    ) -> Result<Node, InsertChildError> {
        let node_id = self.insert_child_impl(node_id, path, kind)?;
        let node = self.0.get(&node_id).unwrap();
        Ok(Node::new(self, Cow::Owned(node_id), node))
    }

    pub fn insert_child_directory(
        &mut self,
        node_id: &NodeId,
        path: impl Into<PathBuf>,
    ) -> Result<DirectoryNode, InsertChildError> {
        let node_id = self.insert_child_impl(node_id, path, PathKind::Directory)?;
        let node = self.0.get(&node_id).unwrap();
        Ok(DirectoryNode::new(self, Cow::Owned(node_id), node))
    }

    pub fn insert_child_file(
        &mut self,
        node_id: &NodeId,
        path: impl Into<PathBuf>,
    ) -> Result<FileNode, InsertChildError> {
        let node_id = self.insert_child_impl(node_id, path, PathKind::File)?;
        let node = self.0.get(&node_id).unwrap();
        Ok(FileNode::new(self, Cow::Owned(node_id), node))
    }

    pub fn insert_path_impl(
        &mut self,
        path: impl AsRef<Path>,
        kind: PathKind,
    ) -> Result<NodeId, InsertChildError> {
        let path = path.as_ref().absolutize().unwrap().to_path_buf();
        let mut current_path = PathBuf::from("/");
        let mut current_node_id = self.root_node_id().clone();
        let components = path.components().skip(1).collect_vec();
        let components_count = components.len();
        for (index, component) in components.into_iter().enumerate() {
            let path::Component::Normal(comp_os_str) = component else {
                unreachable!()
            };
            current_path = current_path.join(comp_os_str);
            let mut children_ids = self.0.children_ids(&current_node_id).unwrap();
            match children_ids
                .find(|node_id| self.0.get(node_id).unwrap().data().name == comp_os_str)
            {
                Some(node_id) => {
                    let node_data = self.0.get(node_id)?.data();
                    if node_data.is_file() {
                        Err(NodeIsNotADirectory(node_data.path.clone()))?
                    }
                    current_node_id = node_id.clone()
                }
                None => {
                    let is_last_component = index == components_count - 1;
                    let component_kind = if kind.is_file() && is_last_component {
                        PathKind::File
                    } else {
                        PathKind::Directory
                    };
                    current_node_id = self
                        .insert_child_unchecked_impl(
                            &current_node_id,
                            current_path.clone(),
                            component_kind,
                        )
                        .unwrap();
                }
            }
        }
        Ok(current_node_id)
    }

    pub fn insert_path(
        &mut self,
        path: impl AsRef<Path>,
        kind: PathKind,
    ) -> Result<Node, InsertChildError> {
        let node_id = self.insert_path_impl(path, kind)?;
        let node = self.0.get(&node_id).unwrap();
        Ok(Node::new(self, Cow::Owned(node_id), node))
    }

    pub fn insert_directory(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<DirectoryNode, InsertChildError> {
        let node_id = self.insert_path_impl(path, PathKind::Directory)?;
        let node = self.0.get(&node_id).unwrap();
        Ok(DirectoryNode::new(self, Cow::Owned(node_id), node))
    }

    pub fn insert_file(&mut self, path: impl AsRef<Path>) -> Result<FileNode, InsertChildError> {
        let node_id = self.insert_path_impl(path, PathKind::File)?;
        let node = self.0.get(&node_id).unwrap();
        Ok(FileNode::new(self, Cow::Owned(node_id), node))
    }

    // XXX not the best, we're cloning node_id, it should take impl Into<NodeId> instead
    fn node_with_id(&self, node_id: impl Borrow<NodeId>) -> Result<Node, NodeIdError> {
        let node_id = Cow::Owned(node_id.borrow().clone());
        let node = self.0.get(&node_id)?;
        Ok(Node {
            tree: self,
            node_id,
            node,
        })
    }

    pub fn node_with_path(&self, path: impl AsRef<Path>) -> Result<Node, PathNotFound> {
        Ok(self.node_with_id(self.path_node_id(path)?).unwrap())
    }

    fn path_node_id(&self, path: impl AsRef<Path>) -> Result<&NodeId, PathNotFound> {
        let path = path.as_ref().absolutize().unwrap();
        let mut current_node_id = self.0.root_node_id().unwrap();
        for component in path.components().skip(1) {
            current_node_id = self
                .0
                .get(current_node_id)
                .unwrap()
                .children()
                .iter()
                .find(|node_id| {
                    let path::Component::Normal(comp_os_str) = component else {
                        unreachable!()
                    };
                    self.0.get(node_id).unwrap().data().name == comp_os_str
                })
                .ok_or_else(|| PathNotFound(path.to_path_buf()))?;
        }
        Ok(current_node_id)
    }

    pub fn node_path(&self, node_id: &NodeId) -> Result<&Path, NodeIdError> {
        Ok(self.0.get(node_id)?.data().path.as_path())
    }

    pub fn contains_path(&self, path: impl AsRef<Path>) -> bool {
        self.path_node_id(path).is_ok()
    }

    pub fn contains_file(&self, path: impl AsRef<Path>) -> bool {
        self.path_node_id(path).map_or(false, |path_node_id| {
            self.0.get(path_node_id).unwrap().data().is_file()
        })
    }

    pub fn contains_directory(&self, path: impl AsRef<Path>) -> bool {
        self.path_node_id(path).map_or(false, |path_node_id| {
            self.0.get(path_node_id).unwrap().data().is_directory()
        })
    }

    pub fn iter(&self) -> NodesIter {
        self.into_iter()
    }

    fn node_count_base(&self, node_id: &NodeId) -> usize {
        self.0.traverse_pre_order_ids(node_id).unwrap().count()
    }

    pub fn node_count(&self) -> usize {
        self.node_count_base(self.root_node_id())
    }

    pub fn path_node_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        let node_id = self.path_node_id(path)?;
        self.check_node_is_directory(node_id)?;
        Ok(self.node_count_base(node_id))
    }

    pub fn directory_count(&self) -> usize {
        self.traverse_directories(TraversalOrder::Pre, true).count()
    }

    pub fn path_directory_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        Ok(self
            .traverse_path_directories(path, TraversalOrder::Pre, true)?
            .count())
    }

    pub fn file_count(&self) -> usize {
        self.traverse_files(TraversalOrder::Pre).count()
    }

    pub fn path_file_count(&self, path: impl AsRef<Path>) -> Result<usize, TraversePathError> {
        Ok(self.traverse_path_files(path, TraversalOrder::Pre)?.count())
    }

    /// checks that the node corresponding to the passed `node_id` is a directory
    /// panics if the node ID is invalid
    fn check_node_is_directory(&self, node_id: &NodeId) -> Result<(), NodeIsNotADirectory> {
        let node = self.0.get(node_id).unwrap();
        if !node.data().is_directory() {
            return Err(NodeIsNotADirectory(node.data().path.clone()));
        }
        Ok(())
    }

    pub fn path_children_nodes_iter(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<NodesIter, PathNotFound> {
        let node_id = self.path_node_id(path)?;
        Ok(NodesIter::children(
            self,
            self.0.children_ids(node_id).unwrap(),
            None,
        ))
    }

    pub fn traverse_nodes(&self, order: TraversalOrder, include_root: bool) -> NodesIter {
        NodesIter::traverse(self, self.root_node_id(), include_root, None, order).unwrap()
    }

    pub fn traverse_path_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<NodesIter, TraversePathError> {
        let node_id = self.path_node_id(path)?;
        self.check_node_is_directory(node_id)?;
        Ok(NodesIter::traverse(self, node_id, include_path, None, order).unwrap())
    }

    pub fn path_children_directories_iter(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<DirectoriesIter, PathNotFound> {
        let node_id = self.path_node_id(path)?;
        Ok(DirectoriesIter::children(
            self,
            self.0.children_ids(node_id).unwrap(),
        ))
    }

    pub fn traverse_directories(
        &self,
        order: TraversalOrder,
        include_root: bool,
    ) -> DirectoriesIter {
        DirectoriesIter::traverse(self, self.root_node_id(), include_root, order).unwrap()
    }

    pub fn traverse_directory_nodes(
        &self,
        order: TraversalOrder,
        include_root: bool,
    ) -> DirectoryNodesIter {
        DirectoryNodesIter::traverse(self, self.root_node_id(), include_root, order).unwrap()
    }

    pub fn traverse_path_directories(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<DirectoriesIter, TraversePathError> {
        let node_id = self.path_node_id(path)?;
        self.check_node_is_directory(node_id)?;
        Ok(DirectoriesIter::traverse(self, node_id, include_path, order).unwrap())
    }

    pub fn traverse_path_directory_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
        include_path: bool,
    ) -> Result<DirectoryNodesIter, TraversePathError> {
        let node_id = self.path_node_id(path)?;
        self.check_node_is_directory(node_id)?;
        Ok(DirectoryNodesIter::traverse(self, node_id, include_path, order).unwrap())
    }

    pub fn path_children_files_iter(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<FilesIter, PathNotFound> {
        let node_id = self.path_node_id(path)?;
        Ok(FilesIter::children(
            self,
            self.0.children_ids(node_id).unwrap(),
        ))
    }

    pub fn path_children_file_nodes_iter(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<FileNodesIter, PathNotFound> {
        let node_id = self.path_node_id(path)?;
        Ok(FileNodesIter::children(
            self,
            self.0.children_ids(node_id).unwrap(),
        ))
    }

    pub fn path_files_iter(
        &self,
        path: impl AsRef<Path>,
        kind: FilesIterKind,
    ) -> Result<FilesIter, TraversePathError> {
        use InternalFilesIterKind::*;
        Ok(match InternalFilesIterKind::from(kind) {
            Children => self.path_children_files_iter(path)?,
            Recursive(order) => self.traverse_path_files(path, order)?,
        })
    }

    pub fn path_file_nodes_iter(
        &self,
        path: impl AsRef<Path>,
        kind: FilesIterKind,
    ) -> Result<FileNodesIter, TraversePathError> {
        use InternalFilesIterKind::*;
        Ok(match InternalFilesIterKind::from(kind) {
            Children => self.path_children_file_nodes_iter(path)?,
            Recursive(order) => self.traverse_path_file_nodes(path, order)?,
        })
    }

    pub fn traverse_files(&self, order: TraversalOrder) -> FilesIter {
        FilesIter::traverse(self, self.root_node_id(), true, order).unwrap()
    }

    pub fn traverse_file_nodes(&self, order: TraversalOrder) -> FileNodesIter {
        FileNodesIter::traverse(self, self.root_node_id(), true, order).unwrap()
    }

    pub fn traverse_path_files(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
    ) -> Result<FilesIter, TraversePathError> {
        let node_id = self.path_node_id(path)?;
        self.check_node_is_directory(node_id)?;
        Ok(FilesIter::traverse(self, node_id, true, order).unwrap())
    }

    pub fn traverse_path_file_nodes(
        &self,
        path: impl AsRef<Path>,
        order: TraversalOrder,
    ) -> Result<FileNodesIter, TraversePathError> {
        let node_id = self.path_node_id(path)?;
        self.check_node_is_directory(node_id)?;
        Ok(FileNodesIter::traverse(self, node_id, true, order).unwrap())
    }

    fn handle_extend_with_dir_error<T, E: Error>(
        value: Result<T, E>,
        error_behavior: ErrorBehavior,
        path: impl AsRef<Path>,
        error_message: &str,
    ) -> anyhow::Result<Option<T>> {
        use ErrorBehavior::*;
        match value {
            Ok(value) => Ok(Some(value)),
            Err(e) => match error_behavior {
                Ignore => Ok(None),
                Display | Stop => {
                    let error_string =
                        format!("{error_message} `{}`: {e}", path.as_ref().to_string_lossy());
                    match error_behavior {
                        Display => {
                            eprintln!("{error_string}");
                            Ok(None)
                        }
                        Stop => Err(anyhow!("{error_string}")),
                        _ => unreachable!(),
                    }
                }
            },
        }
    }

    /// extends the tree with the files in the specified directory, returns an error if the path isn't a directory or does not exist
    /// calls progress for each dir and file found with the total number of directories and total number of files found
    /// returns the total number of directories and total number of files found
    pub fn extend_with_dir_with_progress(
        &mut self,
        dir: impl Into<PathBuf>,
        error_behavior: ErrorBehavior,
        mut progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<(u64, u64)> {
        let dir: PathBuf = dir.into();
        if !dir.is_dir() {
            return Err(anyhow!("not a directory: {}", dir.to_string_lossy()));
        }
        let dir_node_id = self.insert_path_impl(&dir, PathKind::Directory)?;
        let mut dirs_to_process = VecDeque::from_iter([(dir_node_id, dir)]);
        let (mut dir_count, mut file_count): (u64, u64) = (1, 0);
        while let Some((dir_node_id, dir)) = dirs_to_process.pop_back() {
            let read_dir_result = std::fs::read_dir(&dir);
            let Some(dir_iter) = Self::handle_extend_with_dir_error(
                read_dir_result,
                error_behavior,
                &dir,
                "failed to read dir",
            )?
            else {
                continue;
            };
            for entry in dir_iter {
                let entry = entry.map_err(|e| {
                    anyhow!(
                        "error while extending with dir `{}`: {e}",
                        dir.to_string_lossy()
                    )
                })?;
                let Some(file_type) = Self::handle_extend_with_dir_error(
                    entry.file_type(),
                    error_behavior,
                    entry.path(),
                    "failed to get type of file",
                )?
                else {
                    continue;
                };
                if file_type.is_file() {
                    let abs_path = entry.path().absolutize().unwrap().to_path_buf();
                    self.insert_child_unchecked_impl(&dir_node_id, abs_path, PathKind::File)
                        .unwrap();
                    file_count += 1;
                } else if file_type.is_dir() {
                    let abs_path = entry.path().absolutize().unwrap().to_path_buf();
                    let child_dir_node_id = self
                        .insert_child_unchecked_impl(&dir_node_id, abs_path, PathKind::Directory)
                        .unwrap();
                    dirs_to_process.push_front((child_dir_node_id, entry.path()));
                    dir_count += 1;
                }
                progress(dir_count, file_count);
            }
        }
        progress(dir_count, file_count);
        Ok((dir_count, file_count))
    }

    pub fn extend_with_dir(
        &mut self,
        dir: impl Into<PathBuf>,
        error_behavior: ErrorBehavior,
    ) -> anyhow::Result<(u64, u64)> {
        self.extend_with_dir_with_progress(dir, error_behavior, |_, _| {})
    }

    /// extends the tree with the path if it is a file or all the files in path recursively if path is a directory
    /// calls progress for each dir and file found with the total number of directories and total number of files found
    /// returns the total number of directories and total number of files found
    /// returns an error if path does not exist or does not point to a file or directory
    pub fn extend_with_progress(
        &mut self,
        path: impl AsRef<Path>,
        error_behavior: ErrorBehavior,
        mut progress: impl FnMut(u64, u64),
    ) -> anyhow::Result<(u64, u64)> {
        let path = path.as_ref();
        let counts = if path.is_file() {
            self.insert_file(path)?;
            progress(0, 1);
            (0, 1)
        } else if path.is_dir() {
            self.extend_with_dir_with_progress(path, error_behavior, progress)?
        } else {
            return Err(anyhow!(
                "not a normal file or directory: {}",
                path.to_string_lossy()
            ));
        };
        Ok(counts)
    }

    pub fn extend(
        &mut self,
        path: impl AsRef<Path>,
        error_behavior: ErrorBehavior,
    ) -> anyhow::Result<(u64, u64)> {
        self.extend_with_progress(path, error_behavior, |_, _| {})
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TraversalOrder {
    Pre,
    Level,
}

impl TraversalOrder {
    fn iterator<'a>(
        &self,
        tree: &'a FSTree,
        start_node_id: &NodeId,
        include_start_node: bool,
    ) -> Result<TraversalIdsIterator<'a>, NodeIdError> {
        Ok(match self {
            TraversalOrder::Pre => {
                let mut iter = tree.0.traverse_pre_order_ids(start_node_id)?;
                if !include_start_node {
                    iter.next();
                }
                TraversalIdsIterator::PreOrder(iter)
            }
            TraversalOrder::Level => {
                let mut iter = tree.0.traverse_level_order_ids(start_node_id)?;
                if !include_start_node {
                    iter.next();
                }
                TraversalIdsIterator::LevelOrder(iter)
            }
        })
    }
}

enum TraversalIdsIterator<'a> {
    PreOrder(PreOrderTraversalIds<'a, NodeData>),
    LevelOrder(LevelOrderTraversalIds<'a, NodeData>),
    Children(id_tree::ChildrenIds<'a>),
}

impl<'a> Iterator for TraversalIdsIterator<'a> {
    type Item = Cow<'a, NodeId>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self {
            TraversalIdsIterator::PreOrder(iter) => Cow::Owned(iter.next()?),
            TraversalIdsIterator::LevelOrder(iter) => Cow::Owned(iter.next()?),
            TraversalIdsIterator::Children(iter) => Cow::Borrowed(iter.next()?),
        })
    }
}

impl<'a> IntoIterator for &'a FSTree {
    type Item = Node<'a>;

    type IntoIter = NodesIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.traverse_nodes(TraversalOrder::Pre, true)
    }
}

pub struct NodesIter<'a> {
    tree: &'a FSTree,
    kind: Option<NodeKind>,
    iter: TraversalIdsIterator<'a>,
}

impl<'a> NodesIter<'a> {
    fn traverse(
        tree: &'a FSTree,
        start_node_id: &NodeId,
        include_start_node: bool,
        kind: Option<NodeKind>,
        order: TraversalOrder,
    ) -> Result<Self, NodeIdError> {
        Ok(Self {
            tree,
            kind,
            iter: order.iterator(tree, start_node_id, include_start_node)?,
        })
    }

    fn children(
        tree: &'a FSTree,
        children: id_tree::ChildrenIds<'a>,
        kind: Option<NodeKind>,
    ) -> Self {
        Self {
            tree,
            kind,
            iter: TraversalIdsIterator::Children(children),
        }
    }
}

impl<'a> Iterator for NodesIter<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(kind) = self.kind {
            self.iter.find_map(|node_id| {
                let node_data = self.tree.0.get(&node_id).unwrap();
                (node_data.data().kind == kind).then_some(Node {
                    tree: self.tree,
                    node_id,
                    node: node_data,
                })
            })
        } else {
            let node_id = self.iter.next()?;
            let node = self.tree.0.get(node_id.borrow()).unwrap();
            Some(Node {
                tree: self.tree,
                node_id,
                node,
            })
        }
    }
}

pub struct DirectoryNodesIter<'a> {
    tree: &'a FSTree,
    iter: TraversalIdsIterator<'a>,
}

impl<'a> DirectoryNodesIter<'a> {
    fn traverse(
        tree: &'a FSTree,
        start_node_id: &NodeId,
        include_start_node: bool,
        order: TraversalOrder,
    ) -> Result<Self, NodeIdError> {
        Ok(Self {
            tree,
            iter: order.iterator(tree, start_node_id, include_start_node)?,
        })
    }

    fn children(tree: &'a FSTree, children: id_tree::ChildrenIds<'a>) -> Self {
        Self {
            tree,
            iter: TraversalIdsIterator::Children(children),
        }
    }
}

impl<'a> Iterator for DirectoryNodesIter<'a> {
    type Item = DirectoryNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.find_map(|node_id| {
            let node = self.tree.0.get(node_id.borrow()).unwrap();
            node.data()
                .kind
                .is_directory()
                .then(|| DirectoryNode::new(self.tree, node_id, node))
        })
    }
}

pub struct FileNodesIter<'a> {
    tree: &'a FSTree,
    iter: TraversalIdsIterator<'a>,
}

impl<'a> FileNodesIter<'a> {
    fn traverse(
        tree: &'a FSTree,
        start_node_id: &NodeId,
        include_start_node: bool,
        order: TraversalOrder,
    ) -> Result<Self, NodeIdError> {
        Ok(Self {
            tree,
            iter: order.iterator(tree, start_node_id, include_start_node)?,
        })
    }

    fn children(tree: &'a FSTree, children: id_tree::ChildrenIds<'a>) -> Self {
        Self {
            tree,
            iter: TraversalIdsIterator::Children(children),
        }
    }
}

impl<'a> Iterator for FileNodesIter<'a> {
    type Item = FileNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.find_map(|node_id| {
            let node = self.tree.0.get(node_id.borrow()).unwrap();
            node.data()
                .kind
                .is_file()
                .then(|| FileNode::new(self.tree, node_id, node))
        })
    }
}

pub struct DirectoriesIter<'a> {
    tree: &'a FSTree,
    iter: TraversalIdsIterator<'a>,
}

impl<'a> DirectoriesIter<'a> {
    fn traverse(
        tree: &'a FSTree,
        start_node_id: &NodeId,
        include_start_node: bool,
        order: TraversalOrder,
    ) -> Result<Self, NodeIdError> {
        Ok(Self {
            tree,
            iter: order.iterator(tree, start_node_id, include_start_node)?,
        })
    }

    fn children(tree: &'a FSTree, children: id_tree::ChildrenIds<'a>) -> Self {
        Self {
            tree,
            iter: TraversalIdsIterator::Children(children),
        }
    }
}

impl<'a> Iterator for DirectoriesIter<'a> {
    type Item = &'a Path;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.find_map(|node_id| {
            let node_data = self.tree.0.get(node_id.borrow()).unwrap().data();
            node_data
                .kind
                .is_directory()
                .then_some(node_data.path.as_path())
        })
    }
}

pub struct FilesIter<'a> {
    tree: &'a FSTree,
    iter: TraversalIdsIterator<'a>,
}

impl<'a> FilesIter<'a> {
    fn traverse(
        tree: &'a FSTree,
        start_node_id: &NodeId,
        include_start_node: bool,
        order: TraversalOrder,
    ) -> Result<Self, NodeIdError> {
        Ok(Self {
            tree,
            iter: order.iterator(tree, start_node_id, include_start_node)?,
        })
    }

    fn children(tree: &'a FSTree, children: id_tree::ChildrenIds<'a>) -> Self {
        Self {
            tree,
            iter: TraversalIdsIterator::Children(children),
        }
    }
}

impl<'a> Iterator for FilesIter<'a> {
    type Item = &'a Path;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.find_map(|node_id| {
            let node_data = self.tree.0.get(node_id.borrow()).unwrap().data();
            node_data.kind.is_file().then_some(node_data.path.as_path())
        })
    }
}
