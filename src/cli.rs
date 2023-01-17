
use std::path::PathBuf;

use clap::{Parser, Subcommand, Args};
use getset::{Getters, CopyGetters};
use lazy_static::lazy_static;

use crate::{log_level::LogLevel, error_behavior::ErrorBehavior, paths::PathRefs};


#[derive(Parser, Getters, CopyGetters)]
#[clap(author, version, about, long_about = None)]
pub struct Cli {

    /// clones list file in JSON format produced by the `fclones` utility
    #[clap(short, long, env = "CLONES_LIST")]
    #[getset(get = "pub")]
    clones_list: PathBuf,

    /// prune non-existing files and directories from the clones list
    #[clap(short, long)]
    #[getset(get_copy = "pub")]
    prune: bool,

    #[clap(short, long, value_enum, default_value_t = LogLevel::Info)]
    #[getset(get_copy = "pub")]
    log_level: LogLevel,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Args, CopyGetters)]
#[getset(get_copy = "pub")]
pub struct CommonOptions {

    #[clap(short, long, global = true)]
    recursive: bool,

    /// display absolute paths instead of relative paths
    #[clap(short, long, global = true)]
    absolute_paths: bool,

    /// display stats about what is listed
    #[clap(short = 'S', long, global = true)]
    stats: bool,

}

lazy_static! {
    static ref DOT_PATHBUF: PathBuf = PathBuf::from(".");
}

trait CommandArgsInnerPaths {
    fn paths(&self) -> &Vec<PathBuf>;
}

#[derive(Debug, Args)]
pub struct FilesCommandPaths {
    paths: Vec<PathBuf>,
}

impl CommandArgsInnerPaths for FilesCommandPaths {
    fn paths(&self) -> &Vec<PathBuf> {
        &self.paths
    }
}

pub trait CommandArgsPaths {
    fn paths(&self) -> PathRefs;
}

impl<T: CommandArgsInnerPaths> CommandArgsPaths for T {
    fn paths(&self) -> PathRefs {
        if self.paths().is_empty() {
            PathRefs::new(vec![&DOT_PATHBUF])
        } else {
            PathRefs::from_iter(self.paths().iter().map(PathBuf::as_path))
        }
    }
}

#[derive(Debug, Args, Getters)]
pub struct DirsCommandPaths {
    #[clap(value_parser = dir_parser)]
    dirs: Vec<PathBuf>,
}

fn dir_parser(path_str: &str) -> Result<PathBuf, &'static str> {
    let path = PathBuf::from(path_str);
    if ! path.is_dir() {
        return Err("not a directory");
    }
    Ok(path)
}

impl CommandArgsInnerPaths for DirsCommandPaths {
    fn paths(&self) -> &Vec<PathBuf> {
        &self.dirs
    }
}

#[derive(Subcommand)]
pub enum Commands {

    /// list clone or unique dirs
    ///
    /// clone dirs: dirs only containing files which have clones outside of it
    /// unique dirs: dirs only containing files which have no clones outside of it
    Dirs {
        #[clap(flatten)]
        global_options: CommonOptions,

        #[clap(short, long, conflicts_with = "unique", requires = "map")]
        show_refs: bool,

        #[clap(short = 'd', long, conflicts_with = "unique", requires = "map")]
        ref_details: bool,

        /// display what directories outside contain clones from the clone directories
        #[clap(short, long, conflicts_with = "unique")]
        map: bool,

        /// display unique dirs instead of clones
        #[clap(short, long)]
        unique: bool,

        /// use \0 line terminator to print paths so that the output can be piped to `xargs -0`
        #[clap(short = '0', global = true, conflicts_with = "map")]
        null_line_terminator: bool,

        /// specify what to do in case there is an error while listing a directory
        #[clap(short = 'E', long, value_enum, default_value_t = ErrorBehavior::Stop)]
        error_behavior: ErrorBehavior,

        #[clap(flatten)]
        dirs: DirsCommandPaths,
    },

    /// list clone or unique files
    Files {
        #[clap(flatten)]
        global_options: CommonOptions,

        /// display unique files instead of clones
        #[clap(short, long)]
        unique: bool,

        /// display clones in groups (clone groups and inside/outside of specified directory)
        #[clap(short, long, conflicts_with = "unique")]
        map: bool,

        /// display files which have at least one duplicate in the specified path they were found in
        #[clap(short, long, conflicts_with = "unique")]
        inside: bool,

        /// display only files inside specified paths which have at least one duplicate in the same path
        #[clap(short = 'I', long, requires = "map")]
        inside_only: bool,

        /// only display files which have at least one duplicate outside of the specified path they were found in
        #[clap(short, long, conflicts_with = "unique")]
        outside: bool,

        /// use \0 line terminator to print paths so that the output can be piped to `xargs -0`
        #[clap(short = '0', global = true, conflicts_with = "map")]
        null_line_terminator: bool,

        #[clap(flatten)]
        paths: FilesCommandPaths,
    },

}