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
        path: PathBuf,
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
    let command = invocation_message();
    let cli = Cli::parse();
    match cli.command {
        Commands::Init => init(),
        Commands::Add { path, name } => add(path, name, &command),
        Commands::Remove { path } => remove(path, &command),
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
- `dotman add PATH` imports a file or directory.
- `dotman list` shows registered mappings.
- `dotman status` reports link and Git status.
- `dotman restore` recreates registered symbolic links.
- `dotman remove PATH` restores a regular copy and removes management.

Dotman runs `git init` during setup. It commits changes from `add` and
`remove`. Add a remote and push with your preferred Git commands.
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

fn add(path: PathBuf, name: Option<String>, command: &str) -> Result<()> {
    let store = require_store()?;
    let mut registry = load(&store)?;
    let destination_abs = absolute(&path)?;
    let source_meta = fs::symlink_metadata(&destination_abs).context("path does not exist")?;
    if source_meta.file_type().is_symlink() {
        bail!("path root cannot be a symlink");
    }
    if !destination_abs.starts_with(&store.home) {
        bail!("path must be below HOME");
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
    copy_tree(&destination_abs, &managed, &mut HashSet::new())?;
    if let Err(error) =
        remove_tree(&destination_abs).and_then(|_| replace_with_link(&destination_abs, &managed))
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
    git_commit(&store, command, &[INDEX, &managed_rel])
        .context("path is managed, but Git commit failed")?;
    println!("Managed {destination_rel}");
    println!("Committed {command}");
    Ok(())
}

fn remove(path: PathBuf, command: &str) -> Result<()> {
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
    git_commit(&store, command, &[INDEX, &entry.source])
        .context("path is unmanaged, but Git commit failed")?;
    println!("Unmanaged {destination}");
    println!("Committed {command}");
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

fn invocation_message() -> String {
    let arguments = env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    format_invocation(&arguments)
}

fn format_invocation(arguments: &[String]) -> String {
    if arguments.is_empty() {
        "dotman".to_string()
    } else {
        format!("dotman {}", arguments.join(" "))
    }
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

fn git_commit(store: &Store, message: &str, paths: &[&str]) -> Result<()> {
    let mut add = Command::new("git");
    add.arg("-C")
        .arg(&store.root)
        .arg("add")
        .arg("-A")
        .arg("--");
    add.args(paths);
    let add_output = add.output().context("failed to run git add")?;
    if !add_output.status.success() {
        bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&add_output.stderr).trim()
        );
    }

    let mut commit = Command::new("git");
    commit
        .arg("-C")
        .arg(&store.root)
        .arg("commit")
        .arg("-m")
        .arg(message)
        .arg("--");
    commit.args(paths);
    let commit_output = commit.output().context("failed to run git commit")?;
    if !commit_output.status.success() {
        bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit_output.stderr).trim()
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn add_accepts_one_path() {
        let cli = Cli::try_parse_from(["dotman", "add", ".zshrc"]).unwrap();
        match cli.command {
            Commands::Add { path, name } => {
                assert_eq!(path, PathBuf::from(".zshrc"));
                assert_eq!(name, None);
            }
            _ => panic!("expected add command"),
        }
    }

    #[test]
    fn add_rejects_a_second_path() {
        let result = Cli::try_parse_from(["dotman", "add", ".zshrc", ".zshrc"]);
        assert!(result.is_err());
    }

    #[test]
    fn commit_message_uses_the_dotman_command() {
        let arguments = vec!["add".to_string(), "~/.zshrc".to_string()];
        assert_eq!(format_invocation(&arguments), "dotman add ~/.zshrc");
    }

    #[test]
    fn git_commit_records_additions_and_deletions() -> Result<()> {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = env::temp_dir().join(format!("dotman-test-{}-{unique}", std::process::id()));
        fs::create_dir_all(root.join(FILES))?;
        let initialize = Command::new("git").arg("init").arg(&root).status()?;
        if !initialize.success() {
            bail!("git init failed during test");
        }
        let configured_name = Command::new("git")
            .args([
                "-C",
                &root.to_string_lossy(),
                "config",
                "user.name",
                "Dotman Test",
            ])
            .status()?;
        if !configured_name.success() {
            bail!("git user.name setup failed during test");
        }
        let configured_email = Command::new("git")
            .args([
                "-C",
                &root.to_string_lossy(),
                "config",
                "user.email",
                "dotman@example.test",
            ])
            .status()?;
        if !configured_email.success() {
            bail!("git user.email setup failed during test");
        }

        fs::write(root.join(INDEX), "version: 1\nentries: []\n")?;
        fs::write(root.join(FILES).join("zshrc"), "setopt autocd\n")?;
        let store = Store {
            home: root.join("home"),
            registry_path: root.join(INDEX),
            root: root.clone(),
        };
        let result = git_commit(&store, "dotman add ~/.zshrc", &[INDEX, "files/zshrc"]);
        let output = Command::new("git")
            .args(["-C", &root.to_string_lossy(), "log", "-1", "--format=%s"])
            .output()?;
        result?;
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)?.trim(),
            "dotman add ~/.zshrc"
        );

        fs::remove_file(root.join(FILES).join("zshrc"))?;
        fs::write(root.join(INDEX), "version: 1\nentries: []\n")?;
        git_commit(&store, "dotman remove ~/.zshrc", &[INDEX, "files/zshrc"])?;
        let output = Command::new("git")
            .args(["-C", &root.to_string_lossy(), "log", "-1", "--format=%s"])
            .output()?;
        let _ = fs::remove_dir_all(&root);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)?.trim(),
            "dotman remove ~/.zshrc"
        );
        Ok(())
    }
}
