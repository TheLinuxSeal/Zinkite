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
    "zink.io", "zink/io/WalkEntry.zn";
    "zink.io", "zink/io/Walker.zn";
    "zink.cli", "zink/cli/Args.zn";
    "zink.cli", "zink/cli/Colors.zn";
    "zink.cli", "zink/cli/ansi.zn";
    "zink.lang", "zink/lang/Iterator.zn";
    "zink.lang", "zink/lang/LineIterator.zn";
    "zink.lang", "zink/lang/WordIterator.zn";
    "zink.lang", "zink/lang/SplitIterator.zn";
    "zink.lang", "zink/lang/CodePointIterator.zn";
    "zink.lang", "zink/lang/ChunkIterator.zn";
    "zink.lang", "zink/lang/WindowIterator.zn";
    "zink.lang", "zink/lang/env.zn";
    "zink.lang", "zink/lang/EnvVar.zn";
    "zink.lang", "zink/lang/EnvIterator.zn";
    "zink.lang", "zink/lang/TextMethods.zn";
    "zink.time", "zink/time/time.zn";
    "zink.time", "zink/time/Duration.zn";
    "zink.time", "zink/time/Instant.zn";
    "zink.time", "zink/time/Timestamp.zn";
    "zink.time", "zink/time/DateTime.zn";
    "zink.os", "zink/os/Exit.zn";
    "zink.os", "zink/os/process.zn";
    "zink.os", "zink/os/Child.zn";
    "zink.os", "zink/os/Command.zn";
    "zink.os", "zink/os/term.zn";
    "zink.random", "zink/random/Rng.zn";
    "zink.random", "zink/random/random.zn";
    "zink.hash", "zink/hash/hash.zn";
    "zink.hash", "zink/hash/Sha2_224.zn";
    "zink.hash", "zink/hash/Sha2_256.zn";
    "zink.hash", "zink/hash/Sha2_384.zn";
    "zink.hash", "zink/hash/Sha2_512.zn";
    "zink.hash", "zink/hash/Sha2_512_224.zn";
    "zink.hash", "zink/hash/Sha2_512_256.zn";
    "zink.hash", "zink/hash/Sha3_224.zn";
    "zink.hash", "zink/hash/Sha3_256.zn";
    "zink.hash", "zink/hash/Sha3_384.zn";
    "zink.hash", "zink/hash/Sha3_512.zn";
    "zink.hash", "zink/hash/Shake_128.zn";
    "zink.hash", "zink/hash/Shake_256.zn";
    "zink.hash", "zink/hash/Crc32.zn";
    "zink.encoding", "zink/encoding/hex.zn";
    "zink.encoding", "zink/encoding/base64.zn";
    "zink.data", "zink/data/ParseError.zn";
    "zink.data", "zink/data/Kind.zn";
    "zink.data", "zink/data/Node.zn";
    "zink.data", "zink/data/Value.zn";
    "zink.data", "zink/data/Entry.zn";
    "zink.data", "zink/data/ValueIterator.zn";
    "zink.data", "zink/data/EntryIterator.zn";
    "zink.data", "zink/data/Parser.zn";
    "zink.data", "zink/data/DataWriter.zn";
    "zink.json", "zink/json/JsonToken.zn";
    "zink.json", "zink/json/JsonEvent.zn";
    "zink.json", "zink/json/json.zn";
    "zink.json", "zink/json/JsonReader.zn";
    "zink.json", "zink/json/JsonParser.zn";
    "zink.json", "zink/json/JsonWriter.zn";
    "zink.csv", "zink/csv/CsvReader.zn";
    "zink.csv", "zink/csv/CsvWriter.zn";
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
