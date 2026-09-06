use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::symlink;

const INDEX: &str = "index.yaml";
const FILES: &str = "files";
const README: &str = "README.md";

#[derive(Parser)]
#[command(
    name = "dotman",
    version,
    about = "Manage Unix dotfiles with symbolic links"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init,
    Add {
        source: PathBuf,
        destination: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    Remove {
        path: PathBuf,
    },
    List,
    Restore,
    Status,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Registry {
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    source: String,
    destination: String,
}

struct Store {
    home: PathBuf,
    root: PathBuf,
    registry_path: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init => init(),
        Commands::Add {
            source,
            destination,
            name,
        } => add(source, destination, name),
        Commands::Remove { path } => remove(path),
        Commands::List => list(),
        Commands::Restore => restore(),
        Commands::Status => status(),
    }
}

fn store() -> Result<Store> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let root = home.join(".config").join("dotman");
    Ok(Store {
        home,
        registry_path: root.join(INDEX),
        root,
    })
}

fn init() -> Result<()> {
    let store = store()?;
    if store.root.exists() {
        bail!(
            "{} already exists; init will not change it",
            store.root.display()
        );
    }
    fs::create_dir_all(store.root.join(FILES))?;
    write_registry(
        &store,
        &Registry {
            version: 1,
            entries: Vec::new(),
        },
    )?;
    fs::write(store.root.join(README), repository_readme())?;
    let output = Command::new("git")
        .arg("init")
        .arg(&store.root)
        .output()
        .context("failed to run git init")?;
    if !output.status.success() {
        bail!(
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    println!("Initialized {}", store.root.display());
    println!(
        "Next: cd {} && git add index.yaml README.md && git commit -m 'Initialize dotman'",
        store.root.display()
    );
    println!("Then add a remote and push with your preferred Git commands.");
    Ok(())
}

fn repository_readme() -> &'static str {
    "# Dotman repository

This repository is managed by dotman.

Dotman stores managed copies below `files/` and records each installation
mapping in `index.yaml`. It creates symbolic links from installed paths to
those managed copies.

## Basic commands

- `dotman init` creates this repository.
- `dotman add SOURCE DESTINATION` imports a file or directory.
- `dotman list` shows registered mappings.
- `dotman status` reports link and Git status.
- `dotman restore` recreates registered symbolic links.
- `dotman remove PATH` restores a regular copy and removes management.

Dotman runs `git init` during setup. Run Git add, commit, remote, push, and
other Git commands yourself.
"
}

fn load(store: &Store) -> Result<Registry> {
    let text = fs::read_to_string(&store.registry_path)
        .with_context(|| format!("cannot read {}", store.registry_path.display()))?;
    let registry: Registry = serde_yaml::from_str(&text).context("invalid index.yaml")?;
    if registry.version != 1 {
        bail!("unsupported registry version {}", registry.version);
    }
    let mut destinations = HashSet::new();
    let mut names = HashSet::new();
    for entry in &registry.entries {
        validate_relative(&entry.source)?;
        validate_relative(&entry.destination)?;
        if !entry.source.starts_with("files/") {
            bail!("source must be below files/");
        }
        if !destinations.insert(entry.destination.clone()) {
            bail!("duplicate destination: {}", entry.destination);
        }
        if let Some(name) = &entry.name {
            if name.trim().is_empty() {
                bail!("entry names cannot be empty");
            }
            if !names.insert(name) {
                bail!("duplicate entry name: {name}");
            }
        }
    }
    Ok(registry)
}

fn write_registry(store: &Store, registry: &Registry) -> Result<()> {
    let text = serde_yaml::to_string(registry).context("cannot serialize registry")?;
    fs::write(&store.registry_path, text).context("cannot write index.yaml")
}

fn add(source: PathBuf, destination: PathBuf, name: Option<String>) -> Result<()> {
    let store = require_store()?;
    let mut registry = load(&store)?;
    let source_abs = absolute(&source)?;
    let destination_abs = absolute(&destination)?;
    let source_meta = fs::symlink_metadata(&source_abs).context("source does not exist")?;
    if source_meta.file_type().is_symlink() {
        bail!("source root cannot be a symlink");
    }
    if !source_abs.starts_with(&store.home) || !destination_abs.starts_with(&store.home) {
        bail!("source and destination must be below HOME");
    }
    if fs::canonicalize(&source_abs).context("cannot resolve source")?
        != fs::canonicalize(&destination_abs).context("cannot resolve destination")?
    {
        bail!("source and destination must name the same path");
    }
    let destination_rel = destination_abs
        .strip_prefix(&store.home)?
        .to_string_lossy()
        .to_string();
    if registry
        .entries
        .iter()
        .any(|entry| entry.destination == destination_rel)
    {
        bail!("destination is already registered");
    }
    let managed_rel = format!("files/{destination_rel}");
    let managed = store.root.join(&managed_rel);
    if managed.exists() {
        bail!("managed path already exists: {}", managed.display());
    }
    copy_tree(&source_abs, &managed, &mut HashSet::new())?;
    if let Err(error) =
        remove_tree(&source_abs).and_then(|_| replace_with_link(&source_abs, &managed))
    {
        let _ = fs::remove_dir_all(&managed);
        let _ = fs::remove_file(&managed);
        return Err(error);
    }
    registry.entries.push(Entry {
        name,
        source: managed_rel.clone(),
        destination: destination_rel.clone(),
    });
    if let Err(error) = write_registry(&store, &registry) {
        bail!("file linked, but registry update failed: {error}");
    }
    println!("Managed {destination_rel}");
    println!(
        "Next: cd {} && git add index.yaml '{}' && git commit -m 'Add {}'",
        store.root.display(),
        managed_rel,
        destination_rel
    );
    Ok(())
}

fn remove(path: PathBuf) -> Result<()> {
    let store = require_store()?;
    let mut registry = load(&store)?;
    let destination = relative_destination(&store, &path)?;
    let position = registry
        .entries
        .iter()
        .position(|entry| entry.destination == destination)
        .context("path is not registered")?;
    let entry = registry.entries[position].clone();
    let target = store.root.join(&entry.source);
    let install = store.home.join(&entry.destination);
    ensure_link(&install, &target)?;
    let temp = install.with_file_name(format!(".dotman-remove-{}", std::process::id()));
    copy_tree(&target, &temp, &mut HashSet::new())?;
    fs::remove_file(&install).context("cannot remove managed symlink")?;
    fs::rename(&temp, &install).context("cannot restore regular path")?;
    remove_tree(&target)?;
    registry.entries.remove(position);
    write_registry(&store, &registry)?;
    println!("Unmanaged {destination}");
    println!(
        "Next: cd {} && git add -A -- index.yaml '{}' && git commit -m 'Remove {}'",
        store.root.display(),
        entry.source,
        destination
    );
    Ok(())
}

fn list() -> Result<()> {
    let store = require_store()?;
    for entry in load(&store)?.entries {
        println!(
            "{}{} -> {}",
            entry
                .name
                .as_deref()
                .map(|n| format!("[{n}] "))
                .unwrap_or_default(),
            entry.destination,
            entry.source
        );
    }
    Ok(())
}

fn restore() -> Result<()> {
    let store = require_store()?;
    let registry = load(&store)?;
    let mut conflicts = 0;
    for entry in registry.entries {
        let target = store.root.join(&entry.source);
        let install = store.home.join(&entry.destination);
        if install.is_symlink() {
            if link_target(&install)? == target {
                continue;
            }
            println!("Conflict: {} has a different symlink", install.display());
            conflicts += 1;
            continue;
        }
        if install.exists() {
            println!("Conflict: {} exists", install.display());
            conflicts += 1;
            continue;
        }
        if let Err(error) = replace_with_link(&install, &target) {
            println!("Failed: {}: {error}", install.display());
            conflicts += 1;
        }
    }
    if conflicts > 0 {
        bail!("restore completed with {conflicts} conflict(s)");
    }
    println!("All registered links are restored.");
    Ok(())
}

fn status() -> Result<()> {
    let store = require_store()?;
    let registry = load(&store)?;
    let mut links = 0;
    let mut committed_clean = 0;
    for entry in &registry.entries {
        let target = store.root.join(&entry.source);
        let install = store.home.join(&entry.destination);
        if install.is_symlink() && link_target(&install)? == target {
            links += 1;
        }
        if git_committed_clean(&store.root, &entry.source)? {
            committed_clean += 1;
        }
    }
    println!("registered: {}", registry.entries.len());
    println!("healthy symlinks: {links}");
    println!("committed and clean: {committed_clean}");
    Ok(())
}

fn require_store() -> Result<Store> {
    let store = store()?;
    if !store.registry_path.is_file() {
        bail!("dotman is not initialized; run dotman init");
    }
    Ok(store)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    Ok(normalize(&path))
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn validate_relative(path: &str) -> Result<()> {
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("unsafe registry path: {path}");
    }
    Ok(())
}

fn relative_destination(store: &Store, path: &Path) -> Result<String> {
    let abs = absolute(path)?;
    let rel = abs
        .strip_prefix(&store.home)
        .context("path must be below HOME")?;
    Ok(rel.to_string_lossy().to_string())
}

fn copy_tree(source: &Path, destination: &Path, seen: &mut HashSet<PathBuf>) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        let canonical = fs::canonicalize(source)?;
        if !seen.insert(canonical) {
            bail!("symlink cycle detected at {}", source.display());
        }
        fs::create_dir_all(destination)?;
        let mut has_entry = false;
        for child in fs::read_dir(source)? {
            has_entry = true;
            let child = child?;
            copy_tree(&child.path(), &destination.join(child.file_name()), seen)?;
        }
        seen.remove(&fs::canonicalize(source)?);
        if !has_entry {
            bail!("empty directories are not supported");
        }
    } else if metadata.is_file() || metadata.file_type().is_symlink() {
        let resolved = fs::canonicalize(source)?;
        let resolved_meta = fs::metadata(&resolved)?;
        if resolved_meta.is_dir() {
            return copy_tree(&resolved, destination, seen);
        }
        if !resolved_meta.is_file() {
            bail!("unsupported filesystem entry: {}", source.display());
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(resolved, destination)?;
    } else {
        bail!("unsupported filesystem entry: {}", source.display());
    }
    Ok(())
}

fn replace_with_link(path: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() || path.is_symlink() {
        bail!("destination exists: {}", path.display());
    }
    #[cfg(unix)]
    {
        symlink(target, path).with_context(|| format!("cannot create link {}", path.display()))?;
    }
    Ok(())
}

fn link_target(path: &Path) -> Result<PathBuf> {
    Ok(normalize(&fs::read_link(path)?))
}

fn ensure_link(path: &Path, target: &Path) -> Result<()> {
    if !path.is_symlink() || link_target(path)? != target {
        bail!("{} is not the expected dotman symlink", path.display());
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn git_committed_clean(root: &Path, relative: &str) -> Result<bool> {
    let head = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "cat-file",
            "-e",
            &format!("HEAD:{relative}"),
        ])
        .stderr(std::process::Stdio::null())
        .status();
    if !head.map(|status| status.success()).unwrap_or(false) {
        return Ok(false);
    }
    let output = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "status",
            "--porcelain",
            "--",
            relative,
        ])
        .output()?;
    Ok(output.status.success() && output.stdout.is_empty())
}
