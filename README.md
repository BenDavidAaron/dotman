# dotman

`dotman` manages Unix dotfiles with symbolic links and a Git repository.

It copies each managed path into `~/.config/dotman/files/`. It then replaces
the original path with a symbolic link to that copy.

## Requirements

- A Unix system with symbolic link support.
- Rust and Cargo.
- Git in your `PATH`.

## Install

Build and install the command from this repository:

```sh
cargo install --path .
```

Run the development build without installing it:

```sh
cargo run -- --help
```

## Quick start

Create the managed repository:

```sh
dotman init
```

Add a file from your home directory:

```sh
dotman add ~/.zshrc
```

Add a directory from your home directory:

```sh
dotman add ~/.config/nvim --name neovim
```

Commit the managed files:

```sh
cd ~/.config/dotman
git add index.yaml files
git commit -m 'Add dotfiles'
```

Add a remote and push with normal Git commands.

## Commands

| Command | Description |
| --- | --- |
| `dotman init` | Create `~/.config/dotman` and initialize its Git repository. |
| `dotman add PATH [--name NAME]` | Copy a path into the repository and link the original path. |
| `dotman remove PATH` | Restore a regular copy and stop managing the path. |
| `dotman list` | Show each managed path and its stored copy. |
| `dotman restore` | Recreate missing registered symbolic links. |
| `dotman status` | Report registered paths, healthy links, and clean Git entries. |

## Repository layout

```text
~/.config/dotman/
├── index.yaml       # Managed path registry
├── README.md        # Repository-specific command reference
└── files/           # Managed file and directory copies
```

Each registry entry maps a destination below your home directory to a path
below `files/`.

## Safety behavior

`add` accepts only paths below your home directory. It rejects a symbolic link
as the root path. It also rejects an already registered destination.

`restore` does not overwrite existing files or different symbolic links. It
reports each conflict and returns an error.

`remove` requires the expected `dotman` symbolic link. It restores a regular
copy before it removes the managed copy.

Dotman does not commit, push, or configure Git remotes. Review and commit the
repository changes yourself.

Empty directories are not supported. Dotman copies file contents for symbolic
links inside managed directories.

## Move to another computer

Clone your dotman repository into `~/.config/dotman`. Then run:

```sh
dotman restore
```

Resolve any reported conflicts before you run `restore` again.

## Development

Run formatting and tests:

```sh
cargo fmt --check
cargo test
```
