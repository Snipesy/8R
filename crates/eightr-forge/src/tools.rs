//! External tools and the forge cache: downloads (Maven artifacts are immutable, so cached
//! forever by URL), `java`/`javac`, the SDK's `android.jar`, and R8 by version.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, String>;

/// The cache root: `$EIGHTR_FORGE_CACHE`, else `$XDG_CACHE_HOME/8r-forge`, else `~/.cache/8r-forge`.
pub fn cache_root() -> PathBuf {
    if let Some(p) = std::env::var_os("EIGHTR_FORGE_CACHE") {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).unwrap_or_else(|| PathBuf::from("."));
    base.join("8r-forge")
}

pub fn mkdirs(p: &Path) -> Result<()> {
    fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn read(p: &Path) -> Result<Vec<u8>> {
    fs::read(p).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn read_string(p: &Path) -> Result<String> {
    fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn write(p: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    if let Some(d) = p.parent() {
        mkdirs(d)?;
    }
    // Write-then-rename: a crashed or concurrent run never leaves a torn cache file.
    let mut tmp = p.as_os_str().to_owned();
    tmp.push(format!(".tmp{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, p).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        return Err(format!("{cmd:?} failed:\n{s}"));
    }
    Ok(s)
}

/// The cache path of a URL (`https://host/a/b` → `<cache>/net/host/a/b`).
fn url_path(url: &str) -> PathBuf {
    let rest = url.split_once("://").map_or(url, |x| x.1);
    let mut p = cache_root().join("net");
    for part in rest.split('/').filter(|s| !s.is_empty() && *s != "..") {
        p.push(part);
    }
    p
}

/// Downloads `url` into the cache (once); `None` when the server says 404. Other failures are
/// errors, never cached.
pub fn fetch(url: &str) -> Result<Option<PathBuf>> {
    let path = url_path(url);
    let suffixed = |sfx: &str| {
        let mut p = path.clone().into_os_string();
        p.push(sfx);
        PathBuf::from(p)
    };
    let missing = suffixed(".404");
    if path.exists() {
        return Ok(Some(path));
    }
    if missing.exists() {
        return Ok(None);
    }
    if let Some(d) = path.parent() {
        mkdirs(d)?;
    }
    let tmp = suffixed(&format!(".part{}", std::process::id()));
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(2 * attempt));
        }
        let out = Command::new("curl").args(["-sSL", "--retry", "2", "-o"]).arg(&tmp).args(["-w", "%{http_code}"]).arg(url).output().map_err(|e| format!("curl: {e}"))?;
        let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
        match code.as_str() {
            "200" => {
                fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
                return Ok(Some(path));
            }
            "404" => {
                let _ = fs::remove_file(&tmp);
                write(&missing, b"")?;
                return Ok(None);
            }
            _ => last = format!("{url}: HTTP {code} {}", String::from_utf8_lossy(&out.stderr).trim()),
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(last)
}

/// The tools a forge run needs, with their identities (recorded in every pack).
#[derive(Debug, Clone)]
pub struct Tools {
    pub java: PathBuf,
    pub javac: PathBuf,
    pub javac_version: String,
    pub android_jar: PathBuf,
    pub android_jar_sha256: String,
}

impl Tools {
    pub fn find() -> Result<Tools> {
        let sdk = std::env::var_os("ANDROID_HOME")
            .or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Android/Sdk")))
            .ok_or("set ANDROID_HOME")?;
        let android_jar = match std::env::var_os("EIGHTR_ANDROID_JAR") {
            Some(p) => PathBuf::from(p),
            None => newest_platform(&sdk.join("platforms"))?.join("android.jar"),
        };
        let android_jar_sha256 = sha256_hex(&read(&android_jar)?);
        let javac = PathBuf::from("javac");
        let javac_version = run(Command::new(&javac).arg("-version")).map_err(|e| format!("javac not found: {e}"))?.trim().to_string();
        Ok(Tools { java: PathBuf::from("java"), javac, javac_version, android_jar, android_jar_sha256 })
    }
}

fn newest_platform(dir: &Path) -> Result<PathBuf> {
    let mut v: Vec<(u32, PathBuf)> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let n: u32 = p.file_name()?.to_str()?.strip_prefix("android-")?.parse().ok()?;
            p.join("android.jar").exists().then_some((n, p))
        })
        .collect();
    v.sort();
    v.pop().map(|x| x.1).ok_or_else(|| format!("no platforms/android-N/android.jar in {}", dir.display()))
}

/// R8 of `version` from Google Maven: (jar, sha256).
pub fn r8_jar(version: &str) -> Result<(PathBuf, String)> {
    let url = format!("{}/com/android/tools/r8/{version}/r8-{version}.jar", crate::maven::GOOGLE);
    let jar = fetch(&url)?.ok_or_else(|| format!("R8 {version} is not on Google Maven ({url})"))?;
    Ok((jar.clone(), sha256_hex(&read(&jar)?)))
}
