# lsclones

lsclones is a command line utility to list clone and unique files and directories in your filesystem to help with sorting/cleaning.

# Quickstart

The first step is to generate a list of clones which are in the file system tree you are interested in with [fclones](https://github.com/pkolaczk/fclones).
You need to save the list in the JSON format.

`fclones group /a/b/c -o /somewhere/clones_list.json -f json`

You can then use the `lsc` binary provided by this crate to list clones and unique files and directories in /a/b/c.
The most convenient is to set an environment variable to the path of the JSON clones list but you can also specify which
clones list file to use on the command line with the `-c` or `--clones-list` arguments.

With shells like Bash, Zsh, ...
`export CLONES_LIST="/somewhere/clones_list.json"`

With Fish
`set -x CLONES_LIST "/somewhere/clones_list.json"`

You can then start using `lsc`. For example for just listing all the clone files in the `/a/b/c` directory and all subdirectories:
`lsc files -r /a/b/c` or cd first into `/a/b/c` then just run `lsc file -r` as by default it will list content from the current directory.

## Other examples:

### Listing clone files in a directory and map where the clones are inside or outside the specified directory

`lsc files -r --map /a/b/c/d`

### Listing unique files (files which do not have clones inside the directories which were scanned with fclones)

`lsc files -ru /a/b/c/d`

### Listing clone directories (directories which only contain clones which are outside of themselves)

Meaning you can remove clone directories or all the clones of the files inside the directory which are outside of it without losing any data

`lsc dirs -r /a/b/c/d`

### Listing clone directories in groups

`lsc dirs -rm /a/b/c/d`

Also with the `-m`/`--map` option you can add the following options:
* `-s` to display the clones from files inside them are located.
* `-d` to display which files in the directories where the clones are located are missing or extra compared to the clone directory

### Listing unique directories (directories which do not contain clones inside or outside of them)

`lsc dirs -ru`

# Installing on your system

It is recommanded to use the binaries provided on the [releases page](http://github.com/shellixyz/lsclones/releases). Extract the compressed archive and put the binary in a location which is referenced from your PATH environment variable.

## Installing from source

* [Install Rust](https://www.rust-lang.org/tools/install) if you don't have it already
* Run `cargo install --locked --git https://github.com/shellixyz/lsclones`
