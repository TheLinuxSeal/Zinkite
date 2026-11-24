//! Building and running programs (DESIGN §14.4): generating C, compiling it with the
//! system C compiler, and the cache that makes `zink run` fast on repeat runs.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

/// How the generated C names a library to link, one line each at its top (DESIGN §12.6).
pub const LINK_MARK: &str = "/* zink-link: ";

/// The libraries the generated C asks for.
fn links(c: &str) -> Vec<String> {
    c.lines()
        .take_while(|l| l.starts_with(LINK_MARK))
        .filter_map(|l| l.strip_prefix(LINK_MARK)?.strip_suffix(" */"))
        .map(|l| format!("-l{}", l))
        .collect()
}

/// The C compiler: `$CC`, then `clang`, `gcc`, and `cc` as a last resort (DESIGN §14.2).
/// Clang comes first: its code was as fast as gcc's or faster, and it compiles faster
/// at `-O0`, which `zink run` uses.
pub fn c_compiler() -> Result<String, String> {
    if let Ok(cc) = std::env::var("CC")
        && !cc.is_empty()
    {
        return Ok(cc);
    }
    for c in ["clang", "gcc", "cc"] {
        if Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Ok(c.to_string());
        }
    }
    Err("no C compiler found; install gcc or clang, or set $CC".into())
}

/// The flags for an optimized build, or a quick one for `zink run`.
fn flags(cc: &str, optimize: bool) -> Vec<&'static str> {
    // No contraction: a multiply and an add round twice wherever the code runs, in a
    // `simd` function's versions with FMA too (DESIGN §12.8).
    let mut f = vec![
        "-std=gnu11",
        "-w",
        "-fwrapv",
        "-fno-strict-aliasing",
        "-ffp-contract=off",
    ];
    f.push(if optimize { "-O2" } else { "-O0" });
    if optimize && let Some(j) = jcc_flag(cc) {
        f.push(j);
    }
    f
}

/// How this C compiler keeps jumps off 32-byte boundaries on x86-64, if it can (DESIGN
/// §12.5): clang takes the flag itself, gcc passes it to the assembler (binutils 2.34
/// and later). Intel's microcode for the JCC erratum, on Skylake through Cascade Lake
/// and Comet Lake, stops the decoded-instruction cache from holding a jump that
/// crosses or ends on such a boundary, so a hot loop that has one decodes every
/// instruction again. Tested once per run of `zink`, by compiling an empty file with
/// it.
fn jcc_flag(cc: &str) -> Option<&'static str> {
    if !cfg!(target_arch = "x86_64") {
        return None;
    }
    static FLAG: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| {
        [
            "-mbranches-within-32B-boundaries",
            "-Wa,-mbranches-within-32B-boundaries",
        ]
        .into_iter()
        .find(|flag| {
            Command::new(cc)
                .args(["-x", "c", "-c", "-o", "/dev/null", "-", flag])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .and_then(|mut child| {
                    // An empty file: closing stdin is the whole input.
                    drop(child.stdin.take());
                    child.wait()
                })
                .is_ok_and(|s| s.success())
        })
    })
}

/// Compiles C source to an executable at `out`.
pub fn compile(c: &str, out: &Path, optimize: bool) -> Result<(), String> {
    let cc = c_compiler()?;
    let dir = out.parent().unwrap_or(Path::new("."));
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let src = dir.join(format!(".zink-{}-{}.c", std::process::id(), n));
    std::fs::write(&src, c).map_err(|e| format!("can't write {}: {}", src.display(), e))?;
    let result = Command::new(&cc)
        .args(flags(&cc, optimize))
        .arg("-o")
        .arg(out)
        .arg(&src)
        .args(links(c))
        .arg("-lm")
        .output();
    let _ = std::fs::remove_file(&src);
    let output = result.map_err(|e| format!("can't run {}: {}", cc, e))?;
    if !output.status.success() {
        return Err(format!(
            "the C compiler failed on the generated code, which is a bug in zink:\n{}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .take(30)
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(())
}

/// `$XDG_CACHE_HOME/zink`, or `~/.cache/zink`: created with mode 0700, and refused
/// if another user could write to it (DESIGN §14.4).
pub(crate) fn cache_dir() -> Result<PathBuf, String> {
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(
            std::env::var("HOME").map_err(|_| "neither $XDG_CACHE_HOME nor $HOME is set")?,
        )
        .join(".cache"),
    };
    let dir = base.join("zink");
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("can't create {}: {}", dir.display(), e))?;
    }
    let meta =
        std::fs::metadata(&dir).map_err(|e| format!("can't read {}: {}", dir.display(), e))?;
    let probe = dir.join(format!(".probe-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|e| format!("can't write to {}: {}", dir.display(), e))?;
    let mine = std::fs::metadata(&probe)
        .map(|m| m.uid())
        .unwrap_or(u32::MAX);
    let _ = std::fs::remove_file(&probe);
    if meta.uid() != mine || meta.permissions().mode() & 0o022 != 0 {
        return Err(format!(
            "{} isn't owned by you, or others can write to it, so zink won't run binaries from it",
            dir.display()
        ));
    }
    Ok(dir)
}

/// The cached executable for this C, compiling it first if it isn't there yet.
pub fn cached(c: &str, optimize: bool) -> Result<PathBuf, String> {
    let cc = c_compiler()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    c.hash(&mut h);
    flags(&cc, optimize).hash(&mut h);
    cc.hash(&mut h);
    env!("CARGO_PKG_VERSION").hash(&mut h);
    let dir = cache_dir()?;
    let exe = dir.join(format!("{:016x}", h.finish()));
    if exe.exists() {
        return Ok(exe);
    }
    // Written to a temporary name and renamed, so a run never sees half a binary.
    let tmp = dir.join(format!(".tmp-{}-{:016x}", std::process::id(), h.finish()));
    compile(c, &tmp, optimize)?;
    std::fs::rename(&tmp, &exe).map_err(|e| format!("can't write {}: {}", exe.display(), e))?;
    Ok(exe)
}

/// Replaces this process with the program, so it keeps the process id, the standard
/// streams, signals and the exit status (DESIGN §14.4).
pub fn exec(exe: &Path, args: &[String]) -> String {
    use std::os::unix::process::CommandExt;
    let _ = std::io::stdout().flush();
    let err = Command::new(exe).args(args).exec();
    format!("can't run {}: {}", exe.display(), err)
}

pub fn clean_cache() -> Result<usize, String> {
    let dir = cache_dir()?;
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_file() && std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}
