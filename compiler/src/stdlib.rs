//! The standard library's Zinkite sources, built into `zink` (DESIGN §8.4). A program
//! that imports one of these packages gets its files added, with the packages they import
//! in turn. Their C layer is the runtime's `zr_` functions (DESIGN §12.6).

use crate::ast;
use crate::source::SourceFile;

pub struct LibFile {
    pub package: &'static str,
    /// Where it is under `compiler/lib`, and its name in messages.
    pub path: &'static str,
    pub text: &'static str,
}

macro_rules! lib {
    ($($package:literal, $path:literal;)*) => {
        pub const FILES: &[LibFile] = &[
            $(LibFile {
                package: $package,
                path: $path,
                text: include_str!(concat!("../lib/", $path)),
            },)*
        ];
    };
}

lib! {
    "zink.io", "zink/io/File.zn";
    "zink.io", "zink/io/fs.zn";
    "zink.io", "zink/io/path.zn";
    "zink.io", "zink/io/Stat.zn";
    "zink.io", "zink/io/Dir.zn";
    "zink.cli", "zink/cli/Args.zn";
    "zink.lang", "zink/lang/Iterator.zn";
    "zink.lang", "zink/lang/LineIterator.zn";
    "zink.lang", "zink/lang/WordIterator.zn";
    "zink.lang", "zink/lang/SplitIterator.zn";
    "zink.lang", "zink/lang/CodePointIterator.zn";
    "zink.lang", "zink/lang/ChunkIterator.zn";
    "zink.lang", "zink/lang/WindowIterator.zn";
}

/// `zink` and `zink.*`, which only the library's own files may declare.
pub fn reserved(package: &str) -> bool {
    package == "zink" || package.starts_with("zink.")
}

pub fn has_package(p: &str) -> bool {
    FILES.iter().any(|f| f.package == p)
}

/// The packages a file imports, and `zink.lang`, which every file does (DESIGN §8.4), and
/// `zink.io`, which `zink.lang`'s streams need.
pub fn imported_packages(f: &ast::File) -> Vec<String> {
    let mut out: Vec<String> = f
        .imports
        .iter()
        .map(|imp| match imp.kind {
            ast::ImportKind::Wildcard => imp.path.text(),
            _ => imp.path.parts[..imp.path.parts.len().saturating_sub(1)]
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join("."),
        })
        .collect();
    out.push("zink.lang".into());
    // `stdin`, `stdout` and `stderr` are `File`s in every program (STDLIB §2.1), and only
    // the library code a program calls is generated.
    out.push("zink.io".into());
    out
}

/// The library files for these packages, as sources named `<zink>/zink/io/File.zn`.
pub fn sources(package: &str) -> Vec<SourceFile> {
    FILES
        .iter()
        .filter(|f| f.package == package)
        .map(|f| SourceFile::new(format!("<zink>/{}", f.path), f.text))
        .collect()
}
