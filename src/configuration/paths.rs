use directories::ProjectDirs;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const QUALIFIER: &str = "org";
const ORGANIZATION: &str = "BlockBreakers";
const APPLICATION: &str = "blockwallet";

fn project_dirs() -> io::Result<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "could not determine home directory")
    })
}

/// Create a directory owned by this app, readable only by its owner.
///
/// `create_dir_all` uses 0777 & ~umask, typically 0755, so the wallet store's directory was
/// listable by every local account. The store itself is encrypted, but there is no reason to
/// advertise its existence or hand out a copy of the ciphertext to brute-force offline.
fn ensure_dir(path: PathBuf) -> io::Result<PathBuf> {
    fs::create_dir_all(&path)?;
    restrict_dir(&path);
    Ok(path)
}

#[cfg(unix)]
fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) {}

/// Restrict a file this app owns to its owner. Best-effort; a failure here is not worth
/// refusing to save over.
#[cfg(unix)]
pub fn restrict_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
pub fn restrict_file(_path: &Path) {}

fn override_root() -> Option<PathBuf> {
    std::env::var_os("BLOCKWALLET_HOME").map(PathBuf::from)
}

pub fn config_dir() -> io::Result<PathBuf> {
    if let Some(root) = override_root() {
        return ensure_dir(root.join("config"));
    }
    ensure_dir(project_dirs()?.config_dir().to_path_buf())
}

pub fn data_dir() -> io::Result<PathBuf> {
    if let Some(root) = override_root() {
        return ensure_dir(root.join("data"));
    }
    ensure_dir(project_dirs()?.data_dir().to_path_buf())
}

pub fn cache_dir() -> io::Result<PathBuf> {
    if let Some(root) = override_root() {
        return ensure_dir(root.join("cache"));
    }
    ensure_dir(project_dirs()?.cache_dir().to_path_buf())
}

pub fn state_dir() -> io::Result<PathBuf> {
    if let Some(root) = override_root() {
        return ensure_dir(root.join("state"));
    }
    let dirs = project_dirs()?;
    match dirs.state_dir() {
        Some(path) => ensure_dir(path.to_path_buf()),
        None => ensure_dir(dirs.data_dir().join("log")),
    }
}

pub fn wallet_store_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("Config.dic"))
}

/// Light/dark preference. Plaintext and outside the encrypted store, because it has to be
/// readable before the user unlocks in order to theme the unlock screen itself.
pub fn appearance_path() -> io::Result<PathBuf> {
    Ok(config_dir()?.join("appearance"))
}

pub fn network_config_path() -> io::Result<PathBuf> {
    Ok(config_dir()?.join("network.yml"))
}

pub fn log_path() -> io::Result<PathBuf> {
    let path = state_dir()?.join("blockwallet.log");
    if !path.exists() {
        fs::File::create(&path)?;
    }
    Ok(path)
}

pub fn backup_dir() -> io::Result<PathBuf> {
    ensure_dir(data_dir()?.join("backups"))
}

pub fn currency_details_path() -> io::Result<PathBuf> {
    Ok(cache_dir()?.join("CurrencyDetails.json"))
}

pub fn images_path() -> io::Result<PathBuf> {
    let mut candidates = Vec::new();
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Images");
    candidates.push(bundled);
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("Images"));
            candidates.push(dir.join("../share/blockwallet/Images"));
        }
    }
    candidates.push(PathBuf::from("/app/share/blockwallet/Images"));
    candidates.push(PathBuf::from("/usr/share/blockwallet/Images"));
    candidates.push(PathBuf::from("/usr/local/share/blockwallet/Images"));
    for path in candidates {
        if path.is_dir() {
            return Ok(path);
        }
    }
    ensure_dir(data_dir()?.join("images"))
}

pub fn icon_cache_path() -> io::Result<PathBuf> {
    ensure_dir(cache_dir()?.join("icons"))
}

pub fn btc_cache_dir() -> io::Result<PathBuf> {
    ensure_dir(cache_dir()?.join("btc"))
}

/// Monero scan caches: one encrypted file per account. See `xmr_chain::ScanCache`.
pub fn xmr_cache_dir() -> io::Result<PathBuf> {
    ensure_dir(cache_dir()?.join("xmr"))
}

pub fn token_icon_path(symbol: &str) -> PathBuf {
    let file = format!("{}.png", symbol);
    let cached = icon_cache_path()
        .map(|dir| dir.join(&file))
        .unwrap_or_default();
    if cached.is_file() {
        return cached;
    }
    match images_path() {
        Ok(mut bundled) => {
            bundled.push("Icons");
            bundled.push(&file);
            bundled
        }
        Err(_) => cached,
    }
}

/// True when `path` really resolves inside one of this app's own directories.
///
/// Had no callers, and did not do what its name implied: `Path::starts_with` compares
/// components literally without resolving `..`, so `<data_dir>/../../../etc/passwd` passed.
/// Nothing depended on that, but a containment check that can be walked out of is worse than
/// none — the next caller would have trusted it. Canonicalising both sides first is what
/// makes the comparison mean anything.
pub fn is_user_writable(path: &Path) -> bool {
    // An existing path canonicalises directly; for one that does not exist yet, the nearest
    // existing ancestor is what matters, since that is where traversal would have to happen.
    fn resolve(path: &Path) -> Option<PathBuf> {
        let mut current = path;
        loop {
            if let Ok(real) = fs::canonicalize(current) {
                return Some(real);
            }
            current = current.parent()?;
        }
    }

    let Some(target) = resolve(path) else { return false };
    [data_dir(), config_dir(), cache_dir(), state_dir()]
        .into_iter()
        .flatten()
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| target.starts_with(&root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_store_lives_under_xdg_not_next_to_the_binary() {
        let store = wallet_store_path().unwrap();
        assert_eq!(store.file_name().unwrap(), "Config.dic");
        assert!(store.parent().unwrap().exists());

        let mut exe_dir = std::env::current_exe().unwrap();
        exe_dir.pop();
        assert_ne!(store.parent().unwrap(), exe_dir.as_path());
        assert!(!store.to_string_lossy().contains("/Users/andy"));
    }

    #[test]
    fn default_usdc_icon_is_not_a_hardcoded_dev_path() {
        let icon = token_icon_path("USDC");
        assert!(!icon.to_string_lossy().contains("/Users/andy"));
    }

    #[test]
    fn containment_check_is_not_fooled_by_dot_dot() {
        // The whole point of the rewrite: `Path::starts_with` is lexical, so this used to
        // return true for a path that escapes the app's directories entirely.
        let escape = data_dir().unwrap().join("../../../etc/passwd");
        assert!(!is_user_writable(&escape));

        let inside = data_dir().unwrap().join("Config.dic");
        assert!(is_user_writable(&inside));

        assert!(!is_user_writable(Path::new("/etc/passwd")));
    }

    #[test]
    fn images_path_resolves_a_directory() {
        let path = images_path().unwrap();
        assert!(path.is_dir());
        assert!(!path.to_string_lossy().contains("/Users/andy/Documents/Dev"));
    }
}
