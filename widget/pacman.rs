//! What the package manager asks pacman, the AUR and the Arch Linux Archive:
//! the original's `perform_search`, `_fetch_available_versions`,
//! `_find_orphans_thread` and `load_all_flatpak_ids`.

use crate::http::{self, quote, unquote};
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};

/// One search result (the original's `PackageObject`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Package {
    pub name: String,
    pub repo: String,
    pub version: String,
    pub installed: bool,
    pub desc: String,
    pub is_aur: bool,
}

/// `subprocess.run(args, capture_output=True, text=True, env={..., 'LC_ALL': 'C'})`:
/// whether it exited 0, and its output. `None` when it could not run.
fn run_c(program: &str, args: &[&str], c_locale: bool) -> Option<(bool, String)> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if c_locale {
        command.env("LC_ALL", "C");
    }
    let output = command.output().ok()?;
    Some((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

/// `str.strip().split('\n')`.
fn stripped_lines(text: &str) -> Vec<&str> {
    text.trim().split('\n').collect()
}

/// The original `perform_search` for `query` (installed packages when it is
/// empty), with the AUR when `aur`. `stale` is asked where the original
/// checked whether a newer search had started; `None` then. Blocking.
pub fn search(query: &str, aur: bool, stale: &dyn Fn() -> bool) -> Option<Vec<Package>> {
    if stale() {
        return None;
    }
    let mut results = Vec::new();
    let args: Vec<&str> = if query.is_empty() {
        vec!["-Qs"]
    } else {
        vec!["-Ss", query]
    };
    let listed = run_c("pacman", &args, true);
    if stale() {
        return None;
    }
    match listed {
        Some((true, stdout)) if !stdout.is_empty() => {
            if let Err(error) = parse_listing(&stdout, query, &mut results) {
                println!("Repo search error: {error}");
            }
        }
        None => println!("Repo search error: [Errno 2] No such file or directory: 'pacman'"),
        _ => {}
    }

    let installed: HashSet<String> = match run_c("pacman", &["-Qq"], true) {
        Some((true, stdout)) => stripped_lines(&stdout)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        _ => HashSet::new(),
    };
    let existing: HashSet<String> = results
        .iter()
        .map(|package: &Package| package.name.clone())
        .collect();
    if !query.is_empty() && aur {
        match search_aur(query, &installed, &existing, &mut results, stale) {
            Ok(true) => {}
            Ok(false) => return None,
            Err(error) => println!("AUR search error: {error}"),
        }
    }
    if stale() {
        return None;
    }
    if !query.is_empty() {
        let q = query.to_lowercase();
        results.sort_by_cached_key(|package| {
            let name = package.name.to_lowercase();
            let rank = if name == q {
                0
            } else if name.starts_with(&q) {
                1
            } else if name.contains(&q) {
                2
            } else {
                3
            };
            (rank, name)
        });
    }
    Some(results)
}

/// pacman's `-Ss`/`-Qs` listing: `repo/name version [groups] [installed]`
/// lines, each followed by an indented description. Stops at the first line
/// the original could not read (keeping what came before it).
fn parse_listing(stdout: &str, query: &str, results: &mut Vec<Package>) -> Result<(), String> {
    let mut current: Option<Package> = None;
    for line in stripped_lines(stdout) {
        if line.starts_with("    ") {
            if let Some(package) = current.as_mut() {
                package.desc = line.trim().to_owned();
            }
            continue;
        }
        if let Some(package) = current.take() {
            results.push(package);
        }
        let parts: Vec<&str> = line.split(' ').collect();
        let full_name = parts[0];
        let version = parts.get(1).ok_or("list index out of range")?;
        let (repo, name) = if full_name.contains('/') {
            let pieces: Vec<&str> = full_name.split('/').collect();
            match pieces.as_slice() {
                [repo, name] => (*repo, *name),
                _ => {
                    return Err(format!(
                        "too many values to unpack (expected 2, got {})",
                        pieces.len()
                    ));
                }
            }
        } else {
            ("local", full_name)
        };
        current = Some(Package {
            name: name.to_owned(),
            repo: repo.to_owned(),
            version: (*version).to_owned(),
            installed: query.is_empty() || line.contains("[installed]"),
            desc: String::new(),
            is_aur: false,
        });
    }
    if let Some(package) = current {
        results.push(package);
    }
    Ok(())
}

/// The AUR part of `perform_search`: `Ok(false)` when the search went stale
/// meanwhile.
fn search_aur(
    query: &str,
    installed: &HashSet<String>,
    existing: &HashSet<String>,
    results: &mut Vec<Package>,
    stale: &dyn Fn() -> bool,
) -> Result<bool, String> {
    let rpc_url = format!(
        "https://aur.archlinux.org/rpc/?v=5&type=search&arg={}",
        quote(query)
    );
    let body = http::get(&rpc_url, 5)?;
    let text = std::str::from_utf8(&body).map_err(|error| error.to_string())?;
    let data: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if stale() {
        return Ok(false);
    }
    if data.get("type").and_then(Value::as_str) == Some("error") {
        return Ok(true);
    }
    let Some(items) = data.get("results").and_then(Value::as_array) else {
        return Ok(true);
    };
    let q = query.to_lowercase();
    for item in items {
        let name = item.get("Name").and_then(Value::as_str).ok_or("'Name'")?;
        if name.to_lowercase().contains(&q) && !existing.contains(name) {
            let version = item
                .get("Version")
                .and_then(Value::as_str)
                .ok_or("'Version'")?;
            results.push(Package {
                name: name.to_owned(),
                repo: "AUR".into(),
                version: version.to_owned(),
                installed: installed.contains(name),
                desc: item
                    .get("Description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                is_aur: true,
            });
        }
    }
    Ok(true)
}

/// A version the original's Change Version dialog offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    /// The repository, or the (translated) "cache" or "archive".
    pub source: String,
    pub ver: String,
    pub current: bool,
    /// A cached file or an archive URL; `None` for a repository version.
    pub url: Option<String>,
}

/// The original `_fetch_available_versions`: the repositories' versions,
/// the package cache's and the Arch Linux Archive's, newest first (by plain
/// string order, as before). `cache` and `archive` are the translated
/// source names. Blocking.
pub fn versions(package: &str, cache: &str, archive: &str) -> Vec<Version> {
    let mut versions: Vec<Version> = Vec::new();
    let installed = match run_c("pacman", &["-Q", package], true) {
        Some((true, stdout)) => {
            let parts: Vec<&str> = stdout.split_whitespace().collect();
            (parts.len() >= 2).then(|| parts[1].to_owned())
        }
        _ => None,
    };
    let current = |ver: &str| installed.as_deref() == Some(ver);

    if let Some((true, stdout)) = run_c("pacman", &["-Sl"], true) {
        for line in stripped_lines(&stdout) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[1] == package {
                versions.push(Version {
                    source: parts[0].to_owned(),
                    ver: parts[2].to_owned(),
                    current: current(parts[2]),
                    url: None,
                });
            }
        }
    }

    let mut seen: HashSet<String> = versions.iter().map(|version| version.ver.clone()).collect();
    let cache_dir = Path::new("/var/cache/pacman/pkg");
    if cache_dir.is_dir()
        && let Ok(pattern) = Regex::new(&format!(
            r"^{}-(\d.+?)-(x86_64|any)\.pkg\.tar\.(zst|xz|gz)$",
            regex::escape(package)
        ))
        && let Ok(entries) = std::fs::read_dir(cache_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(captures) = pattern.captures(&name) {
                let ver = captures[1].to_owned();
                if seen.insert(ver.clone()) {
                    versions.push(Version {
                        source: cache.to_owned(),
                        current: current(&ver),
                        url: Some(cache_dir.join(&name).to_string_lossy().into_owned()),
                        ver,
                    });
                }
            }
        }
    }

    if let Some(first) = package.chars().next() {
        let archive_url = format!(
            "https://archive.archlinux.org/packages/{}/{package}/",
            first.to_lowercase()
        );
        if let Ok(body) = http::get(&archive_url, 10)
            && let Ok(html) = String::from_utf8(body)
            && let Ok(pattern) = Regex::new(&format!(
                r#"href="({}-([^"]+?)-(x86_64|any)\.pkg\.tar\.[a-z]+)""#,
                regex::escape(package)
            ))
        {
            for captures in pattern.captures_iter(&html) {
                let ver = unquote(&captures[2]);
                if seen.insert(ver.clone()) {
                    versions.push(Version {
                        source: archive.to_owned(),
                        current: current(&ver),
                        url: Some(format!("{archive_url}{}", &captures[1])),
                        ver,
                    });
                }
            }
        }
    }

    versions.sort_by(|a, b| b.ver.cmp(&a.ver));
    versions
}

/// The original `_find_orphans_thread`: `pacman -Qdtq`, stripped.
pub fn orphans() -> String {
    run_c("pacman", &["-Qdtq"], false)
        .map(|(_, stdout)| stdout.trim().to_owned())
        .unwrap_or_default()
}

/// The original `load_all_flatpak_ids`: the remotes' app ids by their last
/// dotted part, lowercased. Blocking.
pub fn flatpak_suffixes() -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    if let Some((true, stdout)) = run_c(
        "flatpak",
        &["remote-ls", "--app", "--columns=application"],
        true,
    ) {
        for id in stdout
            .split('\n')
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let suffix = id.rsplit('.').next().unwrap_or(id).to_lowercase();
            map.entry(suffix).or_default().push(id.to_owned());
        }
    }
    map
}

/// `shutil.which(name) is not None`.
pub fn which(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let candidate = dir.join(name);
            std::fs::metadata(&candidate).is_ok_and(|metadata| {
                use std::os::unix::fs::PermissionsExt;
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listings_are_read_like_the_original() {
        let listing = "core/linux 6.10.1.arch1-1 [installed]\n    The Linux kernel\n\
                       extra/linux-docs 6.10.1.arch1-1\n    Documentation\nlocal-only 1.0\n";
        let mut results = Vec::new();
        parse_listing(listing, "linux", &mut results).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].repo, "core");
        assert!(results[0].installed);
        assert_eq!(results[0].desc, "The Linux kernel");
        assert!(!results[1].installed);
        assert_eq!(results[2].repo, "local");
        let mut installed = Vec::new();
        parse_listing("local/a 1\n    x\n", "", &mut installed).unwrap();
        assert!(installed[0].installed);
        let mut broken = Vec::new();
        assert!(parse_listing("core/a 1\nnoversion\n", "a", &mut broken).is_err());
        assert_eq!(broken.len(), 1);
    }

    #[test]
    fn which_finds_executables() {
        assert!(which("sh"));
        assert!(!which("surely-not-a-command-here"));
    }
}
