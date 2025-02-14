#![allow(dead_code)]

use std::{
    borrow::Borrow,
    cmp::Ord,
    env::current_dir,
    io::{self, Write},
    os::unix::prelude::MetadataExt,
    path::Path,
    process,
};

use clap::Parser;
use cli::{CommandArgsPaths, CommonOptions};
use clones::db::ClonesDB;
use crossterm::cursor;
use env_logger::fmt::Color;
use fs::{
    dir,
    tree::clones::{CloneDir, CloneDirs, RefDir},
};
use itertools::Itertools;
use paths::{Clones, PathRefs, TreeWithProgress};
use size::Size;

mod call_rate_limiter;
mod cli;
mod clones;
mod error_behavior;
mod fs;
mod hash;
mod log_level;
mod path;
mod paths;

use crate::{
    cli::{Cli, Commands},
    error_behavior::ErrorBehavior,
    fs::tree::clones::CloneDirGroups,
};

fn files_command(args: &Commands, clones_db: &ClonesDB) -> anyhow::Result<()> {
    let Commands::Files {
        global_options,
        unique,
        paths,
        map,
        inside,
        outside,
        inside_only,
        null_line_terminator,
    } = args
    else {
        unreachable!()
    };

    eprintln!();

    let paths = paths.paths();
    let recursive = global_options.recursive();
    let display_stats = global_options.stats();

    let current_dir = current_dir().unwrap();
    let path_print_style = PathPrintStyle::new(global_options, current_dir.as_path());

    let mut file_count = 0;
    let mut total_size = 0;

    if *map {
        let mut reclaimable_size = 0;
        let clone_groups = paths.clone_groups(recursive, clones_db);
        for (index, clone_group) in clone_groups.iter().enumerate() {
            if (!(*inside || *inside_only) || clone_group.inside().len() > 1)
                && !(*outside && clone_group.outside().is_empty())
            {
                for file in clone_group.inside().iter() {
                    print_path(file, path_print_style, *null_line_terminator);
                }
                file_count += clone_group.inside().len();
                total_size += clone_group.inside().total_size();
                reclaimable_size += clone_group.inside().reclaimable_size();
                if !(*inside_only || clone_group.outside().is_empty()) {
                    bunt::eprintln!("{$green}=>{/$}");
                    for file in clone_group.outside().iter() {
                        print_path(file, path_print_style, *null_line_terminator);
                    }
                }
                if index < clone_groups.len() - 1 {
                    eprintln!()
                }
            }
        }

        if display_stats {
            eprintln!();
            bunt::eprintln!(
                "{[green]:} {$bold}inside files, total size{/$} {[green]:}{$bold}, reclaimable size{/$} {[green]:}",
                file_count, Size::from_bytes(total_size), Size::from_bytes(reclaimable_size)
            );
        }
    } else if *unique {
        let file_tree = paths.tree_with_progress(ErrorBehavior::Display)?;
        for file in file_tree.unique_files_iter(clones_db).sorted() {
            if global_options.stats() {
                file_count += 1;
                total_size += std::fs::metadata(file)?.size();
            }
            print_path(file, path_print_style, *null_line_terminator);
        }
        if display_stats {
            eprintln!();
            bunt::eprintln!(
                "{[green]:} {$bold}files, total size{/$} {[green]:}",
                file_count,
                size::Size::from_bytes(total_size)
            );
        }
    } else if *inside {
        let (stats, clones) = paths.inside_clones(recursive, clones_db);
        for file in clones.into_iter().sorted() {
            if global_options.stats() {
                file_count += 1;
                total_size += std::fs::metadata(file)?.size();
            }
            print_path(file, path_print_style, *null_line_terminator);
        }
        if display_stats {
            eprintln!();
            bunt::eprintln!(
                "{[green]:} {$bold}files, total size{/$} {[green]:}, {[green]:} {$bold}reclaimable, size{/$} {[green]:}",
                stats.total_count(), stats.total_size_human(), stats.reclaimable_count(), stats.reclaimable_size_human()
            );
        }
    } else {
        let (stats, clones) = paths.clones(recursive, clones_db);
        let file_count = clones.len();
        for file in clones.into_iter().sorted() {
            print_path(file, path_print_style, *null_line_terminator);
        }
        if display_stats {
            eprintln!();
            bunt::eprintln!(
                "{[green]:} {$bold}files, total size{/$} {[green]:}, {[green]:} {$bold}reclaimable, size{/$} {[green]:}",
                file_count, stats.total_size_human(), stats.reclaimable_count(), stats.reclaimable_size_human()
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum PathPrintStyle<'a> {
    Absolute,
    RelativeTo(&'a Path),
}

impl<'a> PathPrintStyle<'a> {
    fn new(global_options: &CommonOptions, current_dir: &'a Path) -> Self {
        if global_options.absolute_paths() {
            PathPrintStyle::Absolute
        } else {
            PathPrintStyle::RelativeTo(current_dir)
        }
    }
}

fn print_path(path: impl AsRef<Path>, style: PathPrintStyle, null_line_terminator: bool) {
    let mut path = path.as_ref();
    if let PathPrintStyle::RelativeTo(prefix) = style {
        path = path.strip_prefix(prefix).unwrap_or(path);
    }
    if null_line_terminator {
        print!("{}\0", path.to_string_lossy());
    } else {
        println!("{}", path.to_string_lossy());
    }
}

fn print_clone_dir_path<'a>(clone_dir: impl Borrow<CloneDir<'a>>, style: PathPrintStyle) {
    let clone_dir = clone_dir.borrow();
    let mut path = clone_dir.path();
    if let PathPrintStyle::RelativeTo(prefix) = style {
        if let Ok(rel_path) = path.strip_prefix(prefix) {
            path = rel_path;
        }
    }
    if let Some(deep_path_rel) = clone_dir.deep_path_rel() {
        bunt::println!(
            "{}{$cyan}/{}{/$}",
            path.to_string_lossy(),
            deep_path_rel.to_string_lossy()
        );
    } else {
        println!("{}", path.to_string_lossy());
    }
}

fn print_ref_dir_path<'a>(
    ref_dir: impl Borrow<RefDir<'a>>,
    style: PathPrintStyle,
    missing_files: usize,
    extra_files: usize,
) {
    let mut path = ref_dir.borrow().path().as_path();
    if let PathPrintStyle::RelativeTo(prefix) = style {
        path = path.strip_prefix(prefix).unwrap_or(path);
    }
    bunt::println!(
        "{} {$bold}({[green]:} missing, {[green]:} extra){/$}",
        path.to_string_lossy(),
        missing_files,
        extra_files
    );
}

#[derive(Debug, Clone, Copy)]
enum RefFileType {
    Extra,
    Missing,
}

fn print_ref_file(path: impl AsRef<Path>, style: PathPrintStyle, ref_file_type: RefFileType) {
    let mut path = path.as_ref();
    if let PathPrintStyle::RelativeTo(prefix) = style {
        path = path.strip_prefix(prefix).unwrap_or(path);
    }
    match ref_file_type {
        RefFileType::Extra => bunt::println!("  {$green}+{/$} {}", path.to_string_lossy()),
        RefFileType::Missing => bunt::println!("  {$red}-{/$} {}", path.to_string_lossy()),
    }
}

#[allow(clippy::too_many_arguments)]
fn dirs_command_clones(
    dirs: PathRefs,
    global_options: &CommonOptions,
    map: bool,
    show_refs: bool,
    ref_details: bool,
    error_behavior: ErrorBehavior,
    clones_db: &ClonesDB,
    null_line_terminator: bool,
) -> anyhow::Result<()> {
    let file_tree = dirs.tree_with_progress(error_behavior)?;
    eprintln!();

    let current_dir = current_dir().unwrap();
    let path_print_style = PathPrintStyle::new(global_options, current_dir.as_path());

    if map {
        let clone_dir_groups = dirs
            .iter()
            .flat_map(|dir| {
                file_tree
                    .clone_dir_groups(dir, clones_db, global_options.recursive())
                    .unwrap()
            })
            .collect::<CloneDirGroups>();

        for (index, clone_dir_group) in clone_dir_groups.iter().enumerate() {
            for dir in clone_dir_group {
                print_clone_dir_path(dir, path_print_style);
            }
            if show_refs || ref_details {
                let ref_dirs = clone_dir_group.ref_dirs();
                if !ref_dirs.is_empty() {
                    bunt::println!("{$green}=>{/$}");
                    for ref_dir in ref_dirs {
                        let missing_files = ref_dir.missing();
                        let extra_files = ref_dir.extra();
                        print_ref_dir_path(
                            &ref_dir,
                            path_print_style,
                            missing_files.len(),
                            extra_files.len(),
                        );
                        if ref_details {
                            for file in missing_files {
                                print_ref_file(file, path_print_style, RefFileType::Missing)
                            }
                            for file in extra_files {
                                print_ref_file(file, path_print_style, RefFileType::Extra)
                            }
                        }
                    }
                }
            }
            if index < clone_dir_groups.len() - 1 {
                println!()
            }
        }

        if global_options.stats() {
            eprintln!();
            bunt::eprintln!(
                "{[green]:} {$bold}dirs, total size{/$} {[green]:}{$bold}, minimum reclaimable{/$} {[green]:}",
                clone_dir_groups.dir_count(), clone_dir_groups.size_human(), clone_dir_groups.minimum_reclaimable_size_human()
            );
        }
    } else {
        let mut clone_dirs = dirs
            .iter()
            .flat_map(|dir| {
                file_tree
                    .clone_dirs(dir, clones_db, global_options.recursive())
                    .unwrap()
            })
            .collect::<CloneDirs>();

        clone_dirs.sort_unstable_by(|cd1, cd2| Ord::cmp(cd1.path(), cd2.path()));
        for dir in &clone_dirs {
            print_path(dir.path(), path_print_style, null_line_terminator);
        }

        if global_options.stats() {
            eprintln!();
            bunt::eprintln!(
                "{[green]:} {$bold}dirs, total size{/$} {[green]:}",
                clone_dirs.len(),
                clone_dirs.size_human()
            );
        }
    }

    Ok(())
}

fn dirs_command_unique(
    dirs: PathRefs,
    global_options: &CommonOptions,
    clones_db: &ClonesDB,
    null_line_terminator: bool,
) -> anyhow::Result<()> {
    let mut udirs = vec![];
    let mut dir_count = 0;
    let mut total_size = 0;

    for dir in dirs {
        for udir in dir::unique_dirs(dir, global_options.recursive(), clones_db)? {
            if global_options.stats() {
                dir_count += 1;
                total_size += dir::size(&udir);
            }
            udirs.push(udir);
        }
    }

    let current_dir = current_dir().unwrap();
    let path_print_style = PathPrintStyle::new(global_options, current_dir.as_path());

    udirs.sort_unstable();
    for udir in udirs {
        print_path(&udir, path_print_style, null_line_terminator);
    }

    if global_options.stats() {
        eprintln!();
        bunt::eprintln!(
            "{[green]:} {$bold}dirs, total size{/$} {[green]:}",
            dir_count,
            size::Size::from_bytes(total_size)
        );
    }

    Ok(())
}

fn dirs_command(args: &Commands, clones_db: &ClonesDB) -> anyhow::Result<()> {
    let Commands::Dirs {
        dirs,
        map,
        show_refs,
        ref_details,
        unique,
        global_options,
        error_behavior,
        null_line_terminator,
    } = args
    else {
        unreachable!()
    };

    let dirs = dirs.paths();

    if *unique {
        dirs_command_unique(dirs, global_options, clones_db, *null_line_terminator)?;
    } else {
        dirs_command_clones(
            dirs,
            global_options,
            *map,
            *show_refs,
            *ref_details,
            *error_behavior,
            clones_db,
            *null_line_terminator,
        )?;
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    ctrlc::set_handler(move || {
        crossterm::execute!(io::stderr(), cursor::Show).unwrap();
        process::exit(1);
    })
    .expect("Error setting Ctrl-C handler");

    let cli = Cli::parse();

    env_logger::builder()
        .format(|buf, record| {
            let level_style = buf.default_level_style(record.level());
            write!(buf, "{:<5}", level_style.value(record.level()))?;
            let mut style = buf.style();
            style.set_color(Color::White).set_bold(true);
            write!(buf, "{}", style.value(" > "))?;
            writeln!(buf, "{}", record.args())
        })
        .parse_filters(cli.log_level().to_string().as_str())
        .init();

    let clones_db = ClonesDB::read_clones_file(cli.clones_list(), cli.prune())?;

    match &cli.command {
        cli::Commands::Dirs { .. } => dirs_command(&cli.command, &clones_db),
        cli::Commands::Files { .. } => files_command(&cli.command, &clones_db),
    }?;

    Ok(())
}
