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

`dotman add` commits the registry and managed path automatically. Add a remote
and push with normal Git commands.

## Commands

| Command | Description |
| --- | --- |
| `dotman init` | Create `~/.config/dotman` and initialize its Git repository. |
| `dotman add PATH [--name NAME]` | Copy, link, and commit a path. |
| `dotman remove PATH` | Restore, unmanage, and commit a path. |
| `dotman list` | Show each managed path and its stored copy. |
| `dotman open` | Start an interactive shell in the dotman repository. |
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

## Edit the repository

Run `dotman open` to start a shell in `~/.config/dotman`.

```sh
dotman open
```

Edit managed files and use Git commands in that shell. Run `exit` when you
finish. Your original shell remains in its original directory.

## Safety behavior

`add` accepts only paths below your home directory. It rejects a symbolic link
as the root path. It also rejects an already registered destination.

`restore` does not overwrite existing files or different symbolic links. It
reports each conflict and returns an error.

`remove` requires the expected `dotman` symbolic link. It restores a regular
copy before it removes the managed copy.

Dotman commits `add` and `remove` changes. It uses the complete `dotman`
command as the commit message. It does not push or configure Git remotes.

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

Install Prek and its Git hook:

```sh
brew install prek
prek install
```

Run all hooks before you commit:

```sh
prek run --all-files
```

Prek runs Cargo formatting, Clippy, and Cargo checks before each commit.

## Releases

Push a Git tag to start the release workflow:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The workflow runs unit tests and creates a GitHub release after all builds pass.
It publishes macOS ARM64, Linux ARM64, and Linux AMD64 archives.
