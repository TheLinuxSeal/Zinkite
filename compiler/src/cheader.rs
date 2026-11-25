//! Reading C headers for `extern import` (DESIGN §12.6). clang dumps what a header
//! declares: functions, typedefs, enum constants, and, through the preprocessor, its
//! macros. Each function whose signature has one clear Zinkite meaning becomes callable
//! as it is; the rest are kept with the reason, for the error that suggests an explicit
//! `extern` declaration.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::token::Prim;
use crate::types::Ty;

/// What a file's `extern import`s declare.
#[derive(Default, Debug)]
pub struct CScope {
    pub headers: Vec<String>,
    pub funcs: HashMap<String, CFunc>,
    /// Integer macros and enum constants.
    pub consts: HashMap<String, i128>,
    /// Opaque struct types reached through pointers, by their C name: `FILE`.
    pub handles: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct CFunc {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// Why it can't be called as declared, when it can't.
    pub why_not: Option<String>,
}

/// The C compiler that reads headers: `clang`, which can dump declarations; `$CC` when
/// that's a clang. Found on the path without running it, so a cached read starts nothing.
fn find_clang() -> Result<PathBuf, String> {
    let on_path = |name: &str| -> Option<PathBuf> {
        if name.contains('/') {
            return std::fs::canonicalize(name).ok();
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
            .and_then(|p| std::fs::canonicalize(p).ok())
    };
    if let Ok(cc) = std::env::var("CC")
        && cc.contains("clang")
        && let Some(p) = on_path(&cc)
    {
        return Ok(p);
    }
    on_path("clang").ok_or_else(|| {
        "reading C headers needs clang, which can list what a header declares; install clang"
            .to_string()
    })
}

fn run(cc: &Path, args: &[&str], input: &str) -> Result<String, String> {
    let cc = cc.display().to_string();
    let cc = cc.as_str();
    use std::io::Write;
    let mut child = Command::new(cc)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run {}: {}", cc, e))?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err
            .lines()
            .find(|l| l.contains("error"))
            .unwrap_or("clang failed")
            .trim()
            .to_string();
        // `<stdin>:1:10: fatal error: 'nope.h' file not found` reads better without the prefix.
        let msg = first
            .split("error: ")
            .nth(1)
            .map(str::to_string)
            .unwrap_or(first);
        return Err(msg);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Reads the headers, as one translation unit would include them. What clang found is
/// cached by the headers' names and the clang binary, and kept while every file it read
/// is unchanged, so a later run reads a few files' sizes and times and starts nothing.
pub fn read(headers: &[String]) -> Result<CScope, String> {
    let cc = find_clang()?;
    let file = cache_file(headers, &cc);
    if let Some(f) = &file
        && let Some(s) = load(f, headers)
    {
        return Ok(s);
    }
    let deps_file = file
        .as_ref()
        .map(|f| f.with_extension(format!("d{}", std::process::id())));
    let scope = read_with(&cc, headers, deps_file.as_deref())?;
    if let (Some(f), Some(d)) = (&file, &deps_file) {
        let deps = std::fs::read_to_string(d).unwrap_or_default();
        let _ = std::fs::remove_file(d);
        store(f, &scope, &dep_files(&deps));
    }
    Ok(scope)
}

/// The cache file for these headers and this clang: in the compiler's cache directory,
/// which is the user's own (`driver::cache_dir`), since a planted one would give the
/// compiler false C signatures. None when there's no cache directory.
fn cache_file(headers: &[String], cc: &Path) -> Option<PathBuf> {
    let dir = crate::driver::cache_dir().ok()?;
    let meta = std::fs::metadata(cc).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // The reader's version: what it makes of a header changes with it.
    "zink-cheader 2".hash(&mut h);
    env!("CARGO_PKG_VERSION").hash(&mut h);
    headers.hash(&mut h);
    cc.hash(&mut h);
    (meta.len(), meta.mtime(), meta.mtime_nsec()).hash(&mut h);
    Some(dir.join(format!("h-{:016x}", h.finish())))
}

/// The files a make-style dependency list names: `-.o: /usr/include/stdio.h \ ...`.
fn dep_files(deps: &str) -> Vec<PathBuf> {
    let body = deps.split_once(':').map_or("", |(_, b)| b);
    body.split_whitespace()
        .filter(|w| *w != "\\")
        .map(PathBuf::from)
        .collect()
}

/// A file's size and modification time, which say whether it changed.
fn stamp(p: &Path) -> Option<(u64, i64, i64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.mtime(), m.mtime_nsec()))
}

/// Writes the scope in a line format, through a temporary file and a rename, so a
/// reader never sees half of it.
fn store(f: &Path, scope: &CScope, deps: &[PathBuf]) {
    use std::fmt::Write as _;
    let mut out = String::from("zink-cheader 1\n");
    for d in deps {
        if let Some((len, s, ns)) = stamp(d) {
            let _ = writeln!(out, "dep {} {} {} {}", len, s, ns, d.display());
        }
    }
    let mut consts: Vec<_> = scope.consts.iter().collect();
    consts.sort();
    for (n, v) in consts {
        let _ = writeln!(out, "const {} {}", n, v);
    }
    let mut handles: Vec<_> = scope.handles.iter().collect();
    handles.sort();
    for h in handles {
        let _ = writeln!(out, "handle {}", h);
    }
    let mut funcs: Vec<_> = scope.funcs.iter().collect();
    funcs.sort_by(|a, b| a.0.cmp(b.0));
    for (n, f) in funcs {
        let mut line = format!("func {} {} {}", n, ty_code(&f.ret), f.params.len());
        for p in &f.params {
            line.push(' ');
            line.push_str(&ty_code(p));
        }
        if let Some(w) = &f.why_not {
            line.push_str(" why ");
            line.push_str(w);
        }
        out.push_str(&line);
        out.push('\n');
    }
    let tmp = f.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, out).is_ok() && std::fs::rename(&tmp, f).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The cached scope, when every file clang read is as it was.
fn load(f: &Path, headers: &[String]) -> Option<CScope> {
    let text = std::fs::read_to_string(f).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "zink-cheader 1" {
        return None;
    }
    let mut scope = CScope {
        headers: headers.to_vec(),
        ..Default::default()
    };
    for line in lines {
        let (kind, rest) = line.split_once(' ')?;
        match kind {
            "dep" => {
                let mut w = rest.splitn(4, ' ');
                let len: u64 = w.next()?.parse().ok()?;
                let s: i64 = w.next()?.parse().ok()?;
                let ns: i64 = w.next()?.parse().ok()?;
                let path = Path::new(w.next()?);
                if stamp(path) != Some((len, s, ns)) {
                    return None;
                }
            }
            "const" => {
                let (n, v) = rest.split_once(' ')?;
                scope.consts.insert(n.to_string(), v.parse().ok()?);
            }
            "handle" => {
                scope.handles.insert(rest.to_string());
            }
            "func" => {
                let (main, why) = match rest.split_once(" why ") {
                    Some((m, w)) => (m, Some(w.to_string())),
                    None => (rest, None),
                };
                let mut w = main.split(' ');
                let name = w.next()?.to_string();
                let ret = parse_ty(w.next()?)?;
                let n: usize = w.next()?.parse().ok()?;
                let params: Vec<Ty> = (0..n)
                    .map(|_| w.next().and_then(parse_ty))
                    .collect::<Option<_>>()?;
                scope.funcs.insert(
                    name,
                    CFunc {
                        params,
                        ret,
                        why_not: why,
                    },
                );
            }
            _ => return None,
        }
    }
    Some(scope)
}

/// A Zinkite type as the cache writes it: the few kinds a C signature maps to.
fn ty_code(t: &Ty) -> String {
    match t {
        Ty::Prim(p) => p.name().to_string(),
        Ty::Void => "void".into(),
        Ty::CHandle(n) => format!("h:{}", n),
        t if t.is_string() => "string".into(),
        _ => "?".into(),
    }
}

fn parse_ty(s: &str) -> Option<Ty> {
    Some(match s {
        "void" => Ty::Void,
        "bool" => Ty::Prim(Prim::Bool),
        "string" => Ty::string(),
        "?" => Ty::Unknown,
        s => match s.strip_prefix("h:") {
            Some(n) => Ty::CHandle(n.to_string()),
            None => Ty::Prim(Prim::from_suffix(s)?),
        },
    })
}

/// Runs clang over the headers: once for the declarations, once for the macros, which
/// also lists every file read into `deps` when given.
fn read_with(cc: &Path, headers: &[String], deps: Option<&Path>) -> Result<CScope, String> {
    let src: String = headers
        .iter()
        .map(|h| format!("#include <{}>\n", h))
        .collect();
    let ast = run(
        cc,
        &[
            "-x",
            "c",
            "-fsyntax-only",
            "-Xclang",
            "-ast-dump",
            "-fno-color-diagnostics",
            "-",
        ],
        &src,
    )?;
    let deps_s = deps.map(|d| d.display().to_string());
    let mut margs = vec!["-x", "c", "-dM", "-E"];
    if let Some(d) = &deps_s {
        margs.extend(["-MD", "-MF", d.as_str()]);
    }
    margs.push("-");
    let macros = run(cc, &margs, &src)?;
    let mut scope = CScope {
        headers: headers.to_vec(),
        ..Default::default()
    };
    let mut typedefs: HashMap<String, String> = HashMap::new();
    let mut records: HashSet<String> = HashSet::new();
    let mut funcs: Vec<(String, String)> = Vec::new();
    let mut last_enum: Option<i128> = None;
    let mut pending: Option<String> = None;
    for line in ast.lines() {
        let top = line.starts_with("|-") || line.starts_with("`-");
        let body = line.trim_start_matches(['|', '`', '-', ' ']);
        let kind = body.split_whitespace().next().unwrap_or("");
        if top
            && kind != "EnumDecl"
            && let Some(n) = pending.take()
        {
            last_enum = Some(last_enum.map_or(0, |v| v + 1));
            scope.consts.insert(n, last_enum.unwrap_or(0));
        }
        match kind {
            "TypedefDecl" if top => {
                if let Some((name, ty)) = name_and_type(body) {
                    typedefs.insert(name, ty);
                }
            }
            "RecordDecl" if top => {
                // `RecordDecl ... struct _IO_FILE definition` or a forward declaration.
                let words: Vec<&str> = body.split_whitespace().collect();
                if let Some(i) = words.iter().position(|w| *w == "struct" || *w == "union")
                    && let Some(n) = words.get(i + 1)
                {
                    records.insert(format!("{} {}", words[i], n));
                }
            }
            "FunctionDecl" if top => {
                if let Some((name, ty)) = name_and_type(body) {
                    funcs.push((name, ty));
                }
            }
            "EnumDecl" if top => {
                if let Some(n) = pending.take() {
                    last_enum = Some(last_enum.map_or(0, |v| v + 1));
                    scope.consts.insert(n, last_enum.unwrap_or(0));
                }
                last_enum = None;
            }
            "EnumConstantDecl" => {
                // An implicit value is one more than the last; an explicit one follows.
                if let Some(n) = pending.take() {
                    last_enum = Some(last_enum.map_or(0, |v| v + 1));
                    scope.consts.insert(n, last_enum.unwrap_or(0));
                }
                if let Some((name, _)) = name_and_type(body) {
                    pending = Some(name);
                }
            }
            "value:" if pending.is_some() => {
                // `value: Int 5`
                if let Some(v) = body
                    .split_whitespace()
                    .nth(2)
                    .and_then(|v| v.parse::<i128>().ok())
                {
                    let n = pending.take().unwrap_or_default();
                    last_enum = Some(v);
                    scope.consts.insert(n, v);
                }
            }
            _ => {}
        }
    }
    if let Some(n) = pending.take() {
        last_enum = Some(last_enum.map_or(0, |v| v + 1));
        scope.consts.insert(n, last_enum.unwrap_or(0));
    }
    let types = Types {
        typedefs: &typedefs,
        records: &records,
    };
    for (name, sig) in funcs {
        let f = types.func(&sig, &mut scope.handles);
        scope.funcs.insert(name, f);
    }
    for line in macros.lines() {
        if let Some(rest) = line.strip_prefix("#define ") {
            let mut parts = rest.splitn(2, ' ');
            let name = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("").trim();
            if name.starts_with('_') || name.contains('(') || scope.consts.contains_key(name) {
                continue;
            }
            if let Some(v) = int_macro(value) {
                scope.consts.insert(name.to_string(), v);
            }
        }
    }
    Ok(scope)
}

/// The name and the quoted type of an AST dump line: `... col:12 puts 'int (const char *)'`,
/// preferring the desugared type after `:` when clang gives one. A typedef of an unnamed
/// enum or struct, as `typedef enum { ... } SDL_bool;`, desugars to its own name
/// (`SDL_bool 'enum SDL_bool':'SDL_bool'`), so the written type is kept for it.
fn name_and_type(body: &str) -> Option<(String, String)> {
    let q = body.find(" '")?;
    let name = body[..q].split_whitespace().last()?.to_string();
    let rest = &body[q + 2..];
    let end = rest.find('\'')?;
    let mut ty = rest[..end].to_string();
    if let Some(d) = rest[end + 1..].strip_prefix(":'")
        && let Some(e) = d.find('\'')
        && d[..e] != name
    {
        ty = d[..e].to_string();
    }
    Some((name, ty))
}

/// An integer macro's value: `0`, `(-1)`, `0x20u`, `8192`.
fn int_macro(v: &str) -> Option<i128> {
    let mut v = v.trim();
    while let Some(inner) = v.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        v = inner.trim();
    }
    let (neg, v) = match v.strip_prefix('-') {
        Some(rest) => (true, rest.trim()),
        None => (false, v),
    };
    let v = v.trim_end_matches(['u', 'U', 'l', 'L']);
    let n = if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        i128::from_str_radix(h, 16).ok()?
    } else if v.len() > 1 && v.starts_with('0') {
        i128::from_str_radix(&v[1..], 8).ok()?
    } else {
        v.parse::<i128>().ok()?
    };
    Some(if neg { -n } else { n })
}

struct Types<'t> {
    typedefs: &'t HashMap<String, String>,
    records: &'t HashSet<String>,
}

enum Place {
    Param,
    Ret,
}

impl Types<'_> {
    /// A function's signature, `int (const char *, int)`, in Zinkite types.
    fn func(&self, sig: &str, handles: &mut HashSet<String>) -> CFunc {
        // `void (int) __attribute__((noreturn))`: attributes don't change what crosses.
        let sig = match sig.find(" __attribute__") {
            Some(i) => &sig[..i],
            None => sig,
        };
        let not = |why: &str, ret: Ty| CFunc {
            params: Vec::new(),
            ret,
            why_not: Some(why.to_string()),
        };
        let Some(open) = sig.find('(') else {
            return not("its type isn't a plain function type", Ty::Unknown);
        };
        if !sig.ends_with(')')
            || sig[open + 1..sig.len() - 1].contains('(') && sig[..open].contains('(')
        {
            return not("its type is too involved for Zinkite to read", Ty::Unknown);
        }
        let ret_s = sig[..open].trim();
        let params_s = &sig[open + 1..sig.len() - 1];
        let ret = match self.ty(ret_s, Place::Ret, handles) {
            Ok(t) => t,
            Err(why) => return not(&format!("it returns {}", why), Ty::Unknown),
        };
        let mut params = Vec::new();
        let parts = split_params(params_s);
        if parts.len() == 1 && parts[0] == "void" {
            return CFunc {
                params,
                ret,
                why_not: None,
            };
        }
        for p in parts {
            if p == "..." {
                return not("it takes a variable number of arguments", ret);
            }
            if p.contains('(') {
                return not("it takes a function pointer", ret);
            }
            match self.ty(&p, Place::Param, handles) {
                Ok(t) => params.push(t),
                Err(why) => return not(&format!("it takes {}", why), ret),
            }
        }
        CFunc {
            params,
            ret,
            why_not: None,
        }
    }

    /// One C type in Zinkite terms, or what makes it ambiguous.
    fn ty(&self, t: &str, place: Place, handles: &mut HashSet<String>) -> Result<Ty, String> {
        let spaced = t.replace('*', " * ");
        let mut words: Vec<&str> = spaced.split_whitespace().collect();
        // Qualifiers after the last `*` are on the pointer itself: `char *restrict`.
        while words.last().is_some_and(|w| is_qual(w)) {
            words.pop();
        }
        if words.last() == Some(&"*") {
            words.pop();
            let is_const = words.contains(&"const");
            let base = strip_quals(&words.join(" "));
            let t = format!("{} *", base);
            if base == "char" {
                return match place {
                    // Text C reads, copied with a NUL for the call (STDLIB §10.6).
                    Place::Param if is_const => Ok(Ty::string()),
                    _ => Err(format!("a `{}`, whose owner and length C doesn't say", t)),
                };
            }
            if let Some(name) = self.record_name(&base) {
                handles.insert(name.clone());
                return Ok(Ty::CHandle(name));
            }
            return Err(format!(
                "a `{}`, which could be one value, an array or something C keeps",
                t
            ));
        }
        let t = strip_quals(&words.join(" "));
        let prim = |p: Prim| Ok(Ty::Prim(p));
        match t.as_str() {
            "void" => Ok(Ty::Void),
            "_Bool" | "bool" => prim(Prim::Bool),
            "char" | "signed char" => prim(Prim::I8),
            "unsigned char" => prim(Prim::U8),
            "short" | "short int" | "signed short" | "signed short int" => prim(Prim::I16),
            "unsigned short" | "unsigned short int" => prim(Prim::U16),
            "int" | "signed" | "signed int" => prim(Prim::I32),
            "unsigned int" | "unsigned" => prim(Prim::U32),
            "long" | "long int" | "signed long" | "signed long int" | "long long"
            | "long long int" | "signed long long" => prim(Prim::I64),
            "unsigned long"
            | "unsigned long int"
            | "unsigned long long"
            | "unsigned long long int" => prim(Prim::U64),
            "__int128" => prim(Prim::I128),
            "unsigned __int128" => prim(Prim::U128),
            "float" => prim(Prim::F32),
            "double" => prim(Prim::F64),
            s if s.starts_with("enum ") => prim(Prim::I32),
            s => match self.typedefs.get(s) {
                Some(target) if target != s => self.ty(target, place, handles),
                _ => Err(format!("a `{}`", s)),
            },
        }
    }

    /// The name a pointed-to struct is known by in Zinkite: its typedef, as `FILE`, or
    /// `struct name`. None when the type isn't a struct.
    fn record_name(&self, base: &str) -> Option<String> {
        if base.starts_with("struct ") || base.starts_with("union ") {
            // Prefer a typedef of it, which is how C code names it.
            let alias = self
                .typedefs
                .iter()
                .filter(|(_, t)| t.as_str() == base)
                .map(|(n, _)| n.clone())
                .min();
            return Some(alias.unwrap_or_else(|| base.replace(' ', "_")));
        }
        match self.typedefs.get(base) {
            Some(target)
                if (target.starts_with("struct ") || target.starts_with("union "))
                    && (self.records.contains(target) || !target.contains('{')) =>
            {
                Some(base.to_string())
            }
            _ => None,
        }
    }
}

fn is_qual(w: &str) -> bool {
    matches!(
        w,
        "const" | "volatile" | "restrict" | "__restrict" | "__restrict__"
    )
}

/// `const`, `volatile` and `restrict` don't change what crosses.
fn strip_quals(t: &str) -> String {
    t.split_whitespace()
        .filter(|w| !is_qual(w))
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_params(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Every Zinkite type that can cross into C in an explicit `extern` declaration's
/// parameters (DESIGN §12.6): numbers, `bool`, text, slices of numbers (their pointer
/// only), and handles.
pub fn crosses_as_param(t: &Ty) -> bool {
    match t {
        Ty::Prim(_) | Ty::CHandle(_) => true,
        Ty::Slice(e) | Ty::View(e) => matches!(**e, Ty::Prim(_)),
        _ => false,
    }
}

pub fn crosses_as_result(t: &Ty) -> bool {
    matches!(t, Ty::Prim(_) | Ty::CHandle(_) | Ty::Void)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_lists() {
        let d = "-.o: /usr/include/stdio.h \\\n  /usr/include/features.h /usr/include/math.h\n";
        assert_eq!(
            dep_files(d),
            vec![
                PathBuf::from("/usr/include/stdio.h"),
                PathBuf::from("/usr/include/features.h"),
                PathBuf::from("/usr/include/math.h"),
            ]
        );
    }

    /// A typedef of an unnamed enum, as SDL writes most of its types, is an enum: its
    /// desugared type is only its own name.
    #[test]
    fn typedefs_of_unnamed_enums() {
        let line = "TypedefDecl 0x5741 <line:180:1, line:184:3> col:3 referenced SDL_bool 'enum SDL_bool':'SDL_bool'";
        assert_eq!(
            name_and_type(line),
            Some(("SDL_bool".to_string(), "enum SDL_bool".to_string()))
        );
        let f = "FunctionDecl 0x57 <a.h:1:1, col:70> col:29 f 'size_t (const char *)':'unsigned long (const char *)'";
        assert_eq!(
            name_and_type(f),
            Some(("f".to_string(), "unsigned long (const char *)".to_string()))
        );
        let mut typedefs = HashMap::new();
        typedefs.insert("SDL_bool".to_string(), "enum SDL_bool".to_string());
        let records = HashSet::new();
        let types = Types {
            typedefs: &typedefs,
            records: &records,
        };
        let f = types.func("int (SDL_bool)", &mut HashSet::new());
        assert_eq!(f.why_not, None);
        assert_eq!(f.params, vec![Ty::Prim(Prim::I32)]);
    }

    /// What `store` writes, `load` reads back, while the files it depends on are unchanged.
    #[test]
    fn the_cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("zink-cheader-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dep = dir.join("fake.h");
        std::fs::write(&dep, "int x;").unwrap();
        let mut scope = CScope::default();
        scope.consts.insert("EOF".into(), -1);
        scope.handles.insert("FILE".into());
        scope.funcs.insert(
            "fopen".into(),
            CFunc {
                params: vec![Ty::string(), Ty::string()],
                ret: Ty::CHandle("FILE".into()),
                why_not: None,
            },
        );
        scope.funcs.insert(
            "printf".into(),
            CFunc {
                params: Vec::new(),
                ret: Ty::Prim(Prim::I32),
                why_not: Some("it takes a variable number of arguments".into()),
            },
        );
        let f = dir.join("h-test");
        store(&f, &scope, std::slice::from_ref(&dep));
        let back = load(&f, &["stdio.h".to_string()]).expect("cached");
        assert_eq!(back.consts.get("EOF"), Some(&-1));
        assert!(back.handles.contains("FILE"));
        let fopen = &back.funcs["fopen"];
        assert_eq!(fopen.params, vec![Ty::string(), Ty::string()]);
        assert_eq!(fopen.ret, Ty::CHandle("FILE".into()));
        assert_eq!(
            back.funcs["printf"].why_not.as_deref(),
            Some("it takes a variable number of arguments")
        );
        // A changed dependency makes the entry stale.
        std::fs::write(&dep, "int x; int y;").unwrap();
        assert!(load(&f, &["stdio.h".to_string()]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
