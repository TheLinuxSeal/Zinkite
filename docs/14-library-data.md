# 14. The library: data

This chapter is a reference for the library's data packages: random numbers, hashes,
hexadecimal and base64, CSV, the parsed tree that every data format shares, and JSON. The
conventions they keep, statuses for every failure and buffers the caller passes, are in
[The library: files, processes, time and the command line](13-library-system.md#how-the-library-behaves).
The design is in [`STDLIB.md` §8 and §9](../STDLIB.md#8-math-random-numbers-hashing-and-encoding-v1).

| Package | Import | What it holds |
|---|---|---|
| `zink.random` | `import zink.random.*;` | `random`, `Rng` |
| `zink.hash` | `import zink.hash.*;` | `hash`, `Crc32`, and a class for each SHA-2 and SHA-3 function |
| `zink.encoding` | `import zink.encoding.*;` | `hex`, `base64`; `hex()` on bytes is in `zink.lang` |
| `zink.csv` | `import zink.csv.*;` | `CsvReader`, `CsvWriter` |
| `zink.data` | `import zink.data.*;` | `Node`, `Value`, `Kind`, `Entry`, `ValueIterator`, `EntryIterator`, `ParseError`, `Parser`, `DataWriter` |
| `zink.json` | `import zink.json.*;` | `JsonParser`, `JsonReader`, `JsonEvent`, `JsonToken`, `JsonWriter`, `json` |

`zink.csv` and `zink.json` use `zink.data`'s classes, so naming `ParseError`, `Value` or
`Node` takes `import zink.data.*;` too.

## Random numbers: `zink.random`

An `Rng` is a pseudo-random generator: the same seed gives the same numbers on every run
and every machine.

```java
import zink.random.*;

void main() {
    Rng r = random.seeded(42);
    println("${r.nextU64()} ${r.nextU64()}");
    println("${r.nextF64()}");
    for (i64 i = 0; i < 10; i++) {
        print("${r.range(1, 7)} ");
    }
    println("");
    u8 b = r.range((u8) 0, (u8) 255);
    i8 c = r.range((i8) -100, (i8) 100);
    i128 huge = r.range((i128) -100000000000000000000000, (i128) 100000000000000000000000);
    println("${b} ${c} ${huge}");
    i64 bad @err = r.range(5, 1);
    println("range(5, 1): ${bad} ${status.name(err)}");
    i64 hits = 0;
    for (i64 i = 0; i < 1000; i++) {
        if (r.chance(0.25)) {
            hits++;
        }
    }
    println("chance(0.25) came up ${hits} times in 1000");
}
```

```output
1546998764402558742 6990951692964543102
0.6800434110281394
6 6 5 5 6 5 4 5 2 5 
81 42 -77186772506954549749129
range(5, 1): 0 INVALID
chance(0.25) came up 276 times in 1000
```

```java
Rng random.seeded(u64 seed)                  // the same sequence on every run
Rng random.fromOs()                          // seeded from getrandom
u64 r.nextU64()
f64 r.nextF64()                              // in [0, 1)
<T implements int> T r.range(T lo, T hi)     // in [lo, hi), unbiased: INVALID, and 0, when lo >= hi
bool r.chance(f64 p)                         // true with probability p

void random.secureBytes(u8[..>]& out, i64 n) // n bytes from the kernel, appended: INVALID for a negative n
u64 random.secureU64()
```

- **`Rng` is xoshiro256\*\***, 32 bytes of state, seeded through SplitMix64 as its authors
  recommend. It is fast and has good statistics, and it is **not for secrets**: anyone
  who sees a few outputs can predict the rest.
- **`range` works for every integer type**, `u8` to `i128` and `u128`, and is unbiased
  (Lemire's method). The range is half-open, so `range(1, 7)` is a die, and the largest
  value of a type can't be reached: `nextU64()` is the full `u64` range.
- **`nextF64` takes the top 53 bits** of `nextU64`, so every value is a multiple of 2⁻⁵³.
- **`chance(p)`** is `nextF64() < p`: `chance(0.0)` is never true and `chance(1.0)` always.
- **The methods change the state**, so they need a variable, or a reference `Rng&` in a
  parameter. A copy of an `Rng` is a second generator in the same state, which gives the
  same numbers:

```java
import zink.random.*;

// Fisher-Yates: every order is equally likely.
void shuffle(string[..~]& xs, Rng& r) {
    for (i64 i = xs.len - 1; i > 0; i--) {
        i64 j = r.range(0, i + 1);
        string t = xs[i];
        xs[i] = xs[j];
        xs[j] = t;
    }
}

void main() {
    string[..~] deck = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];
    Rng r = random.seeded(2026);
    Rng saved = r;
    shuffle(deck, r);
    println("${deck}");
    string[..~] again = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];
    shuffle(again, saved);
    println("the copy deals the same: ${deck == again}");
}
```

```output
[2, Q, 3, A, 10, 5, 6, 7, K, J, 9, 4, 8]
the copy deals the same: true
```

- **For keys, tokens and salts, use `random.secureBytes` and `random.secureU64`**, which
  read the kernel's `getrandom`. It never fails on a running Linux system; it can only
  block briefly early in boot. `random.fromOs()` seeds an `Rng` from it, for numbers that
  differ on every run:

```java
import zink.random.*;

void main() {
    u8[..>] token = [];
    random.secureBytes(token, 16);
    println("token ${token.hex()}");
    Rng r = random.fromOs();
    println("a die: ${r.range(1, 7)}, a key: ${random.secureU64()}");
}
```

## Hashes: `zink.hash`

`hash` has one-shot functions, and each one has a class with the same name capitalized for
data that arrives in pieces.

```java
import zink.hash.*;

void main() {
    string t = "abc";
    println("fnv1a    ${hash.fnv1a(t)}");
    println("crc32    ${hash.crc32(t)}");
    println("sha2_224 ${hash.sha2_224(t).hex()}");
    println("sha2_256 ${hash.sha2_256(t).hex()}");
    println("sha2_512_256 ${hash.sha2_512_256(t).hex()}");
    println("sha3_256 ${hash.sha3_256(t).hex()}");
    println("shake_128, 16 bytes ${hash.shake_128(t, 16).hex()}");
    u8[.] none @err = hash.shake_128(t, -1);
    println("shake_128, -1 bytes: ${none.len} ${status.name(err)}");
}
```

```output
fnv1a    16654208175385433931
crc32    891568578
sha2_224 23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7
sha2_256 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
sha2_512_256 53048e2681941ef99b2e29b76b4c7dabe4c2d0c634fc6d46e0e2f13107e7af23
sha3_256 3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532
shake_128, 16 bytes 5881092dd818bf5cf8a3ddb793fbcba7
shake_128, -1 bytes: 0 INVALID
```

| Function | Result | |
|---|---|---|
| `hash.fnv1a(string bytes)` | `u64` | FNV-1a, 64 bits |
| `hash.crc32(string bytes)` | `u32` | CRC-32 with the IEEE polynomial, as gzip and zip use |
| `hash.sha2_224(string bytes)` | `u8[.28]` | SHA-224 (FIPS 180-4) |
| `hash.sha2_256(string bytes)` | `u8[.32]` | SHA-256 |
| `hash.sha2_384(string bytes)` | `u8[.48]` | SHA-384 |
| `hash.sha2_512(string bytes)` | `u8[.64]` | SHA-512 |
| `hash.sha2_512_224(string bytes)` | `u8[.28]` | SHA-512/224 |
| `hash.sha2_512_256(string bytes)` | `u8[.32]` | SHA-512/256 |
| `hash.sha3_224(string bytes)` | `u8[.28]` | SHA3-224 (FIPS 202) |
| `hash.sha3_256(string bytes)` | `u8[.32]` | SHA3-256 |
| `hash.sha3_384(string bytes)` | `u8[.48]` | SHA3-384 |
| `hash.sha3_512(string bytes)` | `u8[.64]` | SHA3-512 |
| `hash.shake_128(string bytes, i64 n)` | `u8[.]` | SHAKE128, `n` bytes of output: `INVALID`, and none, for a negative `n` |
| `hash.shake_256(string bytes, i64 n)` | `u8[.]` | SHAKE256, the same |

- **Names follow the standards' numbers**, with underscores between them: `sha2_512_224`,
  not `sha512224`.
- **A digest is a fixed-size array, returned by value**, so it never touches a data stack.
  `.hex()` on it gives the usual lowercase text. Only the SHAKE functions, whose length is
  the caller's, build their result in place.
- **They are the runtime's C**, one state for every SHA algorithm and an unrolled Keccak.
- **`hashmap` doesn't use these.** It has its own keyed hash, with a seed per run
  ([Maps, sets, pools and iterators](09-collections.md)).

### Incremental hashing

```java
Sha2_256 h = new Sha2_256();          // and Sha2_224, Sha2_384, Sha2_512, Sha2_512_224,
                                      // Sha2_512_256, Sha3_224, Sha3_256, Sha3_384, Sha3_512
void h.update(string chunk)
u8[.32] h.finish()                    // the digest, sized as the function's; h starts over

Shake_128 x = new Shake_128();        // and Shake_256
void x.update(string chunk)
u8[.] x.finish(i64 n)                 // n bytes; x starts over. INVALID for a negative n, and x is left as it was

Crc32 c = new Crc32();
void c.update(string chunk)
u32 c.finish()                        // the CRC so far; c doesn't start over
```

```java
import zink.hash.*;
import zink.io.*;

void main() {
    u8[..>] dir = [];
    fs.tempDir(dir);
    u8[.] p = path.join(dir, "fox.txt");
    File.writeAll(p, "The quick brown fox jumps over the lazy dog");

    File f = File.open(p);
    Sha2_256 sha = new Sha2_256();
    Crc32 crc = new Crc32();
    u8[..>] piece = [];
    while (true) {
        piece.clear();
        i64 n @err = f.read(piece, 16);
        if (err == status.END) {
            break;
        }
        sha.update(piece);
        crc.update(piece);
    }
    f.close();
    println("${sha.finish().hex()}  ${path.base(p)}");
    println("crc32 ${crc.finish()}, all at once ${hash.crc32(File.readAll(p))}");

    Sha2_256 prefix = new Sha2_256();
    prefix.update("header:");
    Sha2_256 a = prefix;            // a copy holds the state so far
    a.update("one");
    Sha2_256 b = prefix;
    b.update("two");
    println("${a.finish().hex() == hash.sha2_256("header:one").hex()} ${b.finish().hex() == hash.sha2_256("header:two").hex()}");
    fs.removeAll(dir);
}
```

```output
d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592  fox.txt
crc32 1095738169, all at once 1095738169
true true
```

- **The pieces can be any size**, and the digest is the same as for all of them at once.
- **A hash object holds its state inline**, 216 bytes, so a copy is a second hash in the
  same state: hash a shared prefix once, then copy.
- **`Crc32.finish` doesn't start over**, unlike the SHA classes: it gives the CRC so far,
  and `update` goes on from there. Make a new `Crc32` for the next stream.
- `examples/hashtree.zn` hashes every file under a directory with a pool of threads, a
  `Sha2_256` per file.

## Hex and base64: `zink.encoding`

```java
import zink.encoding.*;

void main() {
    println("${"hi".hex()} ${(255).hex()} ${((u8) 10).hex()}");
    u8[..>] raw = [];
    hex.decode("DEADbeef", raw);
    println("${raw.len} bytes: ${raw.hex()}, the first ${raw[0]}");

    u8[.] e = base64.encode("Many hands make light work.");
    u8[..>] back = [];
    base64.decode(e, back);
    println("${e} -> ${back}");
    u8[..>] bin = [];
    bin +> 0xfb;
    bin +> 0xff;
    bin +> 0xfe;
    println("standard ${base64.encode(bin)}, URL-safe ${base64.encodeUrl(bin)}; 'a' is ${base64.encode("a")} and ${base64.encodeUrl("a")}");

    u8[..>] out = [];
    out.append("kept:");
    u8 e1 = @base64.decode("QR==", out);
    u8 e2 = @base64.decode("QQ", out);
    u8 e3 = @base64.decodeUrl("QQ", out);
    u8 e4 = @base64.decodeUrl("QQ==", out);
    u8 e5 = @hex.decode("abc", out);
    println("${status.name(e1)} ${status.name(e2)} ${status.name(e3)} ${status.name(e4)} ${status.name(e5)} '${out}'");
}
```

```output
6869 ff a
4 bytes: deadbeef, the first 222
TWFueSBoYW5kcyBtYWtlIGxpZ2h0IHdvcmsu -> Many hands make light work.
standard +//+, URL-safe -__-; 'a' is YQ== and YQ
INVALID INVALID OK OK INVALID 'kept:AA'
```

```java
u8[.] bytes.hex()                                  // on any u8 array or string: two lowercase digits per byte
void hex.decode(string text, u8[..>]& out)         // appended: INVALID for an odd length or a non-hex digit
u8[.] base64.encode(string bytes)                  // RFC 4648, the standard alphabet, with padding
void base64.decode(string text, u8[..>]& out)      // appended: INVALID
u8[.] base64.encodeUrl(string bytes)               // the URL-safe alphabet, - and _, without padding
void base64.decodeUrl(string text, u8[..>]& out)   // appended; padding is optional: INVALID
```

- **`hex()` depends on the receiver.** On an integer it formats the value, without leading
  zeros, so `((u8) 10).hex()` is `a` ([Text](03-text.md#formatting-numbers)). On a `u8` array or a string it
  encodes each byte as two digits, so `"hi".hex()` is `6869`. `hex()` is in `zink.lang`;
  only `hex.decode` needs the import.
- **`hex.decode` takes either case, and is all or nothing.** It checks the text before it
  appends, so on a bad digit `out` is as it was.
- **base64 decoding is strict, and all or nothing.** `INVALID` is a character outside the
  alphabet, whitespace included, a length that can't be base64, padding that's missing or
  out of place, or leftover bits that aren't zero (RFC 4648 §3.5, so `"QR=="` is
  `INVALID`). `out` is then as it was.
- **`decode` needs the padding, and `decodeUrl` takes it or not.**
- **The base64 loops are the runtime's**, written so the C compiler can vectorize them.

## CSV: `zink.csv`

`CsvReader` reads one record per call, as views into the input wherever it can.

```java
import zink.csv.*;
import zink.data.*;

void main() {
    string text = "name,city,note\nAnn,\"Paris, France\",\"said \"\"hi\"\"\"\n\nBo,Oslo,\"two\nlines\"\r\nCy, Rome ,";
    CsvReader r = new CsvReader(text);
    u8[..>] strings = [];                   // fields that had "" inside quotes, decoded
    string[..~] fields = [];                // cleared by every call, so it must allow removal
    while (true) {
        u8 st = @r.next(fields, strings);
        if (st == status.END) {
            break;
        }
        if (st != 0) {
            ParseError e = r.error();
            println("line ${e.line}, column ${e.column}: ${e.message}");
            return;
        }
        print("${fields.len}:");
        for (string f : fields) {
            print(" [${f}]");
        }
        println("");
    }
    println("decoded into strings: [${strings}]");
}
```

```output
3: [name] [city] [note]
3: [Ann] [Paris, France] [said "hi"]
0:
3: [Bo] [Oslo] [two
lines]
3: [Cy] [ Rome ] []
decoded into strings: [said "hi"]
```

```java
CsvReader r = new CsvReader(string input)
void r.next(string[..~]& fields, u8[..>]& strings)   // END after the last record; INVALID
ParseError r.error()                                 // how the last failure happened; code 0 when none did

class CsvReader { string input; u8 separator = ','; u8 quote = '"'; bool strict = false; ... }
```

- **`next` clears `fields`**, then appends one view per field. Clearing is a removal, so
  `fields` must allow removal at the end (`string[..~]`).
- **A field is a view into the input whenever it can be**: an unquoted field, or a quoted
  one with no `""` inside, without its quotes.
- **Only fields with doubled quotes are decoded**, into `strings`. `next` never clears
  `strings`, so every field stays valid; clear it yourself once no earlier field is kept.
- **Spaces are kept**: ` Rome ` is a field with a space at each end.
- **A blank line is a record with no fields**, as Python's `csv` reads it. A line that ends
  in the separator has an empty last field.
- **The format is RFC 4180's**: quoted fields may hold the separator and line breaks, lines
  end in `\n` or `\r\n`, and a UTF-8 byte-order mark at the start is skipped.
- **`separator` and `quote` can be any byte**: `'\t'` for tab-separated values, `';'` for
  the files spreadsheets write in many locales.
- **It fails with `INVALID`** for a quote that is never closed, or for text after a closing
  quote (`"a"b`). With `strict` set, every row must also have as many fields as the first.
  `r.error()` says where, with the line and column counting from 1 and the column in
  bytes.

```java
import zink.csv.*;
import zink.data.*;

void readAll(CsvReader& r) {
    u8[..>] strings = [];
    string[..~] fields = [];
    while (true) {
        u8 st = @r.next(fields, strings);
        if (st == status.END) {
            return;
        }
        if (st != 0) {
            ParseError e = r.error();
            println("  ${status.name(e.code)} at line ${e.line}, column ${e.column}: ${e.message}");
            return;
        }
        println("  ${fields}");
    }
}

void main() {
    println("a quote never closed:");
    CsvReader a = new CsvReader("a,\"b\nc");
    readAll(a);
    println("text after a closing quote:");
    CsvReader b = new CsvReader("a,\"b\"c,d\n");
    readAll(b);
    println("strict:");
    CsvReader c = new CsvReader("a,b\n1,2\n3\n");
    c.strict = true;
    readAll(c);
    println("semicolons, single quotes and a byte-order mark:");
    CsvReader d = new CsvReader("\u{feff}x;'y;z'\n1;''''\n");
    d.separator = ';';
    d.quote = '\'';
    readAll(d);
}
```

```output
a quote never closed:
  INVALID at line 1, column 3: this quote is never closed
text after a closing quote:
  INVALID at line 1, column 6: text after a closing quote
strict:
  [a, b]
  [1, 2]
  INVALID at line 3, column 1: this row has a different number of fields from the first
semicolons, single quotes and a byte-order mark:
  [x, y;z]
  [1, ']
```

### Writing CSV: `CsvWriter`

```java
import zink.csv.*;

void main() {
    u8[..>] out = [];
    CsvWriter w = new CsvWriter(out);
    w.writeRow(["name", "note"]);
    w.writeField("Ann");
    w.writeField("said \"hi\", twice");
    w.endRow();
    w.writeRow(["Bo", " padded "]);
    w.writeRow(["two\nlines", ""]);
    w.writeRow([""]);
    print(out);

    out.clear();
    CsvWriter t = new CsvWriter(out);
    t.separator = '\t';
    t.crlf = true;
    t.writeRow(["a,b", "c\td"]);
    println("tab-separated, as hex: ${out.hex()}");
}
```

```output
name,note
Ann,"said ""hi"", twice"
Bo," padded "
"two
lines",
""
tab-separated, as hex: 612c620922630964220d0a
```

```java
CsvWriter w = new CsvWriter(u8[..>]& out)   // appends to out
void w.writeField(string field)
void w.endRow()
void w.writeRow(string[:] fields)           // each field, then endRow

class CsvWriter { u8 separator = ','; bool crlf = false; ... }
```

- **A field is quoted only when it needs to be**: when it holds the separator, a quote, `\r`
  or `\n`, or starts or ends with a space. A quote inside is doubled.
- **A row of one empty field is written `""`**, so it doesn't read back as a blank line,
  which is a record with no fields.
- **Rows end in `\n`.** `w.crlf = true` gives RFC 4180's `\r\n`.
- **It always quotes with `"`**; `separator` can be any byte.
- **It sets no status.** It holds a reference to `out`, which must outlive it; the bytes are
  there to read as soon as each call returns. Write them with `File.writeAll` when done.
- **Fields round-trip**: what `CsvWriter` writes, `CsvReader` reads back as the same
  fields.

## Parsed trees: `zink.data`

Every data format parses into the same tree, so code that reads a configuration doesn't
depend on the format the file was in, and converting between formats is a parse and a
write. JSON is the format written so far.

```java
import zink.data.*;
import zink.json.*;

void main() {
    string text = "{\n  \"server\": {\"host\": \"example.org\", \"port\": 8080},\n  \"users\": [{\"name\": \"ann\"}, {\"name\": \"b\\u00f6\"}, {}],\n  \"debug\": false\n}";
    u8[..>] strings = [];                   // strings that had escapes, decoded
    Node[..>] tree = [];
    JsonParser p = new JsonParser(text);
    Value cfg @bad = p.parse(tree, strings);
    if (bad) {
        ParseError e = p.error();
        println("config.json:${e.line}:${e.column}: ${e.message}");
        return;
    }

    string host @_ = cfg.get("server").get("host").asStringOr("localhost");
    i64 port @badPort = cfg.get("server").get("port").asI64();      // MISSING, TYPE, RANGE
    bool debug @_ = cfg.get("debug").asBoolOr(false);
    i64 timeout @_ = cfg.get("server").get("timeout").asI64Or(30);
    println("${host}:${port} (${status.name(badPort)}), debug ${debug}, timeout ${timeout}");
    for (Value user : cfg.get("users").items()) {
        string name @_ = user.get("name").asStringOr("?");
        print("${name} ");
    }
    println("");
    println("${tree.len} nodes; strings holds '${strings}'");
}
```

```output
example.org:8080 (OK), debug false, timeout 30
ann bö ? 
18 nodes; strings holds 'bö'
```

**Memory**: three buffers, all the caller's.
- **`text`**, the input. Strings without escapes, and every number, are views into it.
- **`strings`**, where strings with escapes are decoded. Most keys and values have none,
  so it usually stays small: above, only `"b\u00f6"` went there.
- **`tree`**, an array of fixed-size `Node`s, one per key or value, in document order.
- **Views stay valid** because all three only grow while the tree is in use. A `Value` is
  rooted at `tree`, and through it at `text` and `strings`.
- **To parse many documents, declare the buffers inside the loop**, as JSON Lines below
  does. Its data stack frees them with one store at the end of each pass. Clearing `tree`
  and then `strings` instead is an error, since the rules are per variable and can't see
  that `tree` no longer views `strings` ([Memory and references](05-memory.md)):

```java
import zink.data.*;
import zink.json.*;

void main() {
    string text = "{\"n\": 1}\n{\"n\": 2}\n";
    u8[..>] strings = [];
    Node[..~] tree = [];
    for (string line : text.lines()) {
        tree.clear();
        strings.clear();
        JsonParser p = new JsonParser(line);
        Value v @bad = p.parse(tree, strings);
        println("${v.get("n").asI64Or(0)}");
    }
}
```

```error
`clear` removes from `strings` while `tree` may still refer into it
```

### Navigation never fails

- **A missing key or index gives an absent value**, and so does `get` on something that
  isn't an object, or `at` on something that isn't an array.
- **Only reading a value sets a status.** So a chain such as
  `cfg.get("server").get("port").asI64()` needs one check, at the end, and drops no status
  on the way.
- **`items()` and `entries()` are lazy iterators**, a `ValueIterator` and an
  `EntryIterator`, which implement `Iterator<Value>` and `Iterator<Entry>`. A for-each
  loops over one; kept in a variable, one is stepped with `next()` until `END`
  ([Iterators](09-collections.md#iterators)).

`Value` is a view of one node, `{ Node[:] tree; i64 index; }`, with `index` -1 for an
absent value.

| Method | Result | Fails with |
|---|---|---|
| `Kind kind()` | `NULL`, `BOOL`, `INT`, `FLOAT`, `STRING`, `DATETIME`, `ARRAY`, `OBJECT`, or `ABSENT`; `KEY` is a node's kind only | |
| `bool exists()` | false for an absent value | |
| `bool isNull()` | | |
| `Value get(string key)` | an object's value for `key`; the last one when the key repeats | |
| `Value at(i64 i)` | an array's element `i`, from 0; O(i), so iterate with `items()` | |
| `Value pointer(string p)` | an RFC 6901 path, such as `"/servers/0/port"`; `""` is the value itself | |
| `i64 len()` | an array's elements or an object's entries; 0 for anything else | |
| `ValueIterator items()` | an array's elements; none for anything else | |
| `EntryIterator entries()` | an object's entries in document order, repeated keys included, as `Entry { string key; Value value; }` | |
| `string asString()` | | `MISSING`, `TYPE` |
| `i64 asI64()`, `u64 asU64()` | | `MISSING`, `TYPE`, `RANGE` |
| `f64 asF64()` | an integer converts; a number beyond `f64`'s range reads as ±infinity | `MISSING`, `TYPE` |
| `bool asBool()` | | `MISSING`, `TYPE` |
| `DateTime asDateTime()` | a `DATETIME`, which JSON doesn't have | `MISSING`, `TYPE` |
| `asStringOr(fallback)`, `asI64Or`, `asF64Or`, `asBoolOr` | the fallback when the value is absent or null | `TYPE`, with the fallback; `asI64Or` also `RANGE` |
| `string text()` | a scalar as written: a number's digits, even for one too big for `i64`; `"null"` for a null | `MISSING`, and `TYPE` for an object or array |
| `i64 line()` | the line it starts on in the input; 0 for an absent value | |
| `string tag()` | its YAML tag; `""` for every other format | |

```java
import zink.data.*;
import zink.json.*;

void main() {
    string text = "{\"n\": 42, \"f\": 3.0, \"s\": \"8080\", \"big\": 99999999999999999999, \"neg\": -7, \"nil\": null, \"a/b\": 1, \"m~n\": 2, \"dup\": 1, \"dup\": 2, \"list\": [10, 20, 30]}";
    u8[..>] strings = [];
    Node[..>] tree = [];
    JsonParser p = new JsonParser(text);
    Value v = p.parse(tree, strings);

    i64 a @e1 = v.get("n").asI64();
    i64 b @e2 = v.get("f").asI64();
    i64 c @e3 = v.get("s").asI64();
    i64 d @e4 = v.get("big").asI64();
    i64 e @e5 = v.get("nope").asI64();
    u64 f @e6 = v.get("neg").asU64();
    println("asI64: ${a} ${status.name(e1)}, ${b} ${status.name(e2)}, ${c} ${status.name(e3)}, ${d} ${status.name(e4)}, ${e} ${status.name(e5)}; asU64 of -7: ${status.name(e6)}");
    f64 x @e7 = v.get("n").asF64();
    string big @_ = v.get("big").text();
    println("asF64 of 42: ${x} ${status.name(e7)}; the big one's text: ${big}");
    i64 o1 @k1 = v.get("nope").asI64Or(5);
    i64 o2 @k2 = v.get("nil").asI64Or(5);
    i64 o3 @k3 = v.get("s").asI64Or(5);
    println("asI64Or(5): absent ${o1} ${status.name(k1)}, null ${o2} ${status.name(k2)}, a string ${o3} ${status.name(k3)}");
    println("dup ${v.get("dup").asI64Or(0)}, /a~1b ${v.pointer("/a~1b").asI64Or(0)}, /m~0n ${v.pointer("/m~0n").asI64Or(0)}, /list/2 ${v.pointer("/list/2").asI64Or(0)}, /list/02 ${v.pointer("/list/02").exists()}");
    println("at(1) ${v.get("list").at(1).asI64Or(0)}, at(9) ${v.get("list").at(9).exists()}, a long chain ${v.get("a").get("b").at(3).get("c").exists()}");
    println("len ${v.len()} ${v.get("list").len()} ${v.get("n").len()}, isNull ${v.get("nil").isNull()}, nil exists ${v.get("nil").exists()}");
}
```

```output
asI64: 42 OK, 0 TYPE, 0 TYPE, 0 RANGE, 0 MISSING; asU64 of -7: RANGE
asF64 of 42: 42.0 OK; the big one's text: 99999999999999999999
asI64Or(5): absent 5 OK, null 5 OK, a string 5 TYPE
dup 2, /a~1b 1, /m~0n 2, /list/2 30, /list/02 false
at(1) 20, at(9) false, a long chain false
len 11 3 0, isNull true, nil exists true
```

- **`TYPE` is never a silent conversion.** The string `"8080"` isn't an `i64`, and
  `asI64()` on `3.0` fails: a number with a fraction or an exponent is a `FLOAT`.
- **Numbers keep their text.** `asI64`, `asU64` and `asF64` convert on demand, so a number
  beyond `i64` isn't lost: `text()` returns its digits, and a writer copies them as they
  were.
- **`get` scans the object's entries**, which is fast for the small objects that
  configuration files have. For a big object read many times, build a
  `hashmap<string, Value>` from `entries()` once:

```java
import zink.data.*;
import zink.json.*;

void main() {
    string text = "{\"a\": 1, \"b\": 2, \"c\": 3}";
    u8[..>] strings = [];
    Node[..>] tree = [];
    JsonParser p = new JsonParser(text);
    Value v = p.parse(tree, strings);
    hashmap<string, Value> byKey = [];
    for (Entry e : v.entries()) {
        byKey.put(e.key, e.value);
    }
    println("${byKey.get("b").asI64Or(0)} of ${byKey.len} keys");
}
```

```output
2 of 3 keys
```

### The nodes

`Node`s are in document order, and each one knows the size of its subtree, so navigating
is index arithmetic and nothing recurses.

```java
class Node {
    Kind kind;        // u8
    u8 flags;         // how a number or string was written, for the formats that keep it; 0 in JSON
    u16 column;       // where it starts, clamped at 65535
    u32 size;         // nodes in this subtree, itself included: the next sibling is at index + size
    u32 count;        // an array's elements or an object's entries
    u32 line;
    string text;      // a key, a string, or a number's, boolean's or null's text as written
}

enum Kind { ABSENT, NULL, BOOL, INT, FLOAT, STRING, DATETIME, ARRAY, OBJECT, KEY }
```

- **An object's children are entries**, each a `KEY` node followed by its value's subtree.
- **An enum can't be printed yet**, so code that shows a kind switches on it.

```java
import zink.data.*;
import zink.json.*;

string kindName(Kind k) {
    return switch (k) {
        case ABSENT => "ABSENT";
        case NULL => "NULL";
        case BOOL => "BOOL";
        case INT => "INT";
        case FLOAT => "FLOAT";
        case STRING => "STRING";
        case DATETIME => "DATETIME";
        case ARRAY => "ARRAY";
        case OBJECT => "OBJECT";
        case KEY => "KEY";
    };
}

void main() {
    string text = "{\"b\": [1, 2.50, {\"c\": null}], \"a\": \"x\"}";
    u8[..>] strings = [];
    Node[..>] tree = [];
    JsonParser p = new JsonParser(text);
    Value v = p.parse(tree, strings);
    for (i64 i = 0; i < tree.len; i++) {
        Node& n = tree[i];
        println("${i}: ${kindName(n.kind)} size ${n.size} count ${n.count} at ${n.line}:${n.column} '${n.text}'");
    }
}
```

```output
0: OBJECT size 10 count 2 at 1:1 ''
1: KEY size 1 count 0 at 1:2 'b'
2: ARRAY size 6 count 3 at 1:7 ''
3: INT size 1 count 0 at 1:8 '1'
4: FLOAT size 1 count 0 at 1:11 '2.50'
5: OBJECT size 3 count 1 at 1:17 ''
6: KEY size 1 count 0 at 1:18 'c'
7: NULL size 1 count 0 at 1:23 'null'
8: KEY size 1 count 0 at 1:31 'a'
9: STRING size 1 count 0 at 1:36 'x'
```

### `ParseError`

```java
class ParseError { u8 code; i64 line; i64 column; string message; }
```

- **`code` is the status the parse failed with**, or 0 if it didn't fail.
- **`line` and `column` count from 1**, and the column counts bytes.
- **`message` is static text**, such as `"expected ',' or '}'"`, so a `ParseError` can be
  kept as long as needed.

### The interfaces: `Parser` and `DataWriter`

```java
interface Parser {
    mut Value parse(Node[..>]& tree, u8[..>]& strings);
    ParseError error();
}

interface DataWriter {
    mut void beginObject();
    mut void beginArray();
    mut void end();                    // closes the innermost object or array
    mut void key(string k);
    mut void write(string s);
    mut void write(i64 n);
    mut void write(u64 n);
    mut void write(f64 x);
    mut void write(bool b);
    mut void write(DateTime d);
    mut void write(Value v);           // a whole parsed subtree
    mut void writeNull();
    mut void writeNumber(string text); // a number exactly as written, such as the text a reader gave
    mut void finish();                 // the only one that sets a status
}
```

- **`Parser` is for choosing a format at run time**, such as by file extension, through a
  `Parser&`. `JsonParser` is the one written.
- **`DataWriter` is for code written once for every output format**: a generic function
  bounded by it writes JSON through a `JsonWriter` today, and other formats when their
  writers are written.
- **`write` is overloaded**: `w.write(80)` writes an `i64`, a `u32` goes to `write(u64)`, and
  an `f32` to `write(f64)`.

```java
import zink.data.*;
import zink.json.*;

class Server {
    string host;
    i64 port;
}

<W implements DataWriter> void writeServer(W& w, Server s) {
    w.beginObject();
    w.key("host");
    w.write(s.host);
    w.key("port");
    w.write(s.port);
    w.end();
}

void main() {
    u8[..>] out = [];
    JsonWriter w = new JsonWriter(out);
    writeServer(w, new Server("example.org", 8080));
    w.finish();
    println(out);

    string text = "[1, 2]";
    u8[..>] strings = [];
    Node[..>] tree = [];
    JsonParser jp = new JsonParser(text);
    Parser& p = jp;
    Value v @bad = p.parse(tree, strings);
    println("${v.len()} elements, error code ${p.error().code}");
}
```

```output
{"host":"example.org","port":8080}
2 elements, error code 0
```

## JSON: `zink.json`

The format is RFC 8259. `JsonParser` builds a tree; `JsonReader` gives the same input as a
stream of events, for input too big for a tree or for reading straight into a program's
own classes; `JsonWriter` writes JSON into a buffer.

### `JsonParser`

```java
JsonParser p = new JsonParser(string input)
Value p.parse(Node[..>]& tree, u8[..>]& strings)   // INVALID or FULL, with an absent value
ParseError p.error()

class JsonParser { string input; i64 maxDepth = 1024; ... }
```

- **Nesting deeper than `maxDepth` fails with `FULL`.** The parser doesn't recurse: it is
  built on `JsonReader`'s events, which keep their own stack, so a deeply nested file can't
  overflow the native stack.
- **Duplicate keys are kept**, and `get` returns the last one, as JavaScript and Python do.
  `entries()` shows them all.
- **`\u` escapes are decoded to UTF-8**, surrogate pairs included. A lone surrogate
  (`"\ud800"`) becomes U+FFFD.
- **Strings must be valid UTF-8**, and a byte-order mark at the start is skipped.
  Anything other than whitespace after the value is `INVALID`.
- **After a failure, `tree` keeps the nodes appended before it**, and the value is absent.

```java
import zink.data.*;
import zink.json.*;

void parse(string text) {
    u8[..>] strings = [];
    Node[..>] tree = [];
    JsonParser p = new JsonParser(text);
    Value v @bad = p.parse(tree, strings);
    if (bad) {
        ParseError e = p.error();
        println("${status.name(bad)} at ${e.line}:${e.column}: ${e.message}");
        return;
    }
    u8[..>] out = [];
    json.write(out, v, 0);
    println("ok: ${out}");
}

void main() {
    parse("");
    parse("[1, 2");
    parse("{\"a\" 1}");
    parse("{\"a\": 1,}");
    parse("[1 2]");
    parse("01");
    parse("[tru]");
    parse("\"a\\x\"");
    parse("\"never closed");
    parse("{} {}");
    parse("{a: 1}");
    parse("\"\\ud83d\\ude00 \\u00e9 \\ud800\"");
}
```

```output
INVALID at 1:1: the input ends before a value
INVALID at 1:6: the input ends inside an object or array
INVALID at 1:6: expected ':' after the key
INVALID at 1:9: expected a key in double quotes
INVALID at 1:4: expected ',' or ']'
INVALID at 1:2: text after the value
INVALID at 1:2: expected a value
INVALID at 1:3: an unknown escape
INVALID at 1:1: this string is never closed
INVALID at 1:4: text after the value
INVALID at 1:2: expected a key in double quotes
ok: "😀 é �"
```

### `JsonReader`, `JsonEvent` and `JsonToken`

```java
JsonReader r = new JsonReader(string input)
JsonEvent r.next()                     // END once the value is complete; INVALID or FULL at an error
void r.skipValue()                     // skips the value that comes next, a whole object or array included
ParseError r.error()

class JsonReader { string input; i64 maxDepth = 1024; ... }
class JsonEvent { JsonToken kind; string text; i64 line; i64 column; }
string e.decode(u8[..>]& strings)      // the text with its escapes decoded
i64 e.asI64()                          // TYPE for anything but a NUMBER without a fraction or exponent; RANGE
f64 e.asF64()                          // TYPE for anything but a NUMBER

enum JsonToken { BEGIN_OBJECT, END_OBJECT, BEGIN_ARRAY, END_ARRAY, KEY, STRING, NUMBER, TRUE, FALSE, NULL }
```

```java
import zink.json.*;

string name(JsonToken t) {
    return switch (t) {
        case BEGIN_OBJECT => "BEGIN_OBJECT";
        case END_OBJECT => "END_OBJECT";
        case BEGIN_ARRAY => "BEGIN_ARRAY";
        case END_ARRAY => "END_ARRAY";
        case KEY => "KEY";
        case STRING => "STRING";
        case NUMBER => "NUMBER";
        case TRUE => "TRUE";
        case FALSE => "FALSE";
        case NULL => "NULL";
    };
}

void main() {
    JsonReader r = new JsonReader("{\"id\": 7,\n \"tags\": [\"caf\\u00e9\", true, null],\n \"skip\": {\"x\": [1, 2]}, \"f\": -2.5e3}");
    u8[..>] strings = [];
    while (JsonEvent e : r.next()) {
        print("${e.line}:${e.column} ${name(e.kind)} '${e.text}'");
        if (e.kind == JsonToken.STRING) {
            print(", decoded '${e.decode(strings)}'");
        }
        if (e.kind == JsonToken.NUMBER) {
            i64 n @notInt = e.asI64();
            print(", asI64 ${n} ${status.name(notInt)}, asF64 ${e.asF64()}");
        }
        println("");
        if (e.kind == JsonToken.KEY && e.text == "skip") {
            r.skipValue();
        }
    }
    println("error code ${r.error().code}");

    JsonReader bad = new JsonReader("[1, 2,, 3]");
    i64 count = 0;
    while (JsonEvent e : bad.next()) @why {
        count++;
    }
    println("${count} events, then ${status.name(why)}: ${bad.error().message} at column ${bad.error().column}");
}
```

```output
1:1 BEGIN_OBJECT '{'
1:2 KEY 'id'
1:8 NUMBER '7', asI64 7 OK, asF64 7.0
2:2 KEY 'tags'
2:10 BEGIN_ARRAY '['
2:11 STRING 'caf\u00e9', decoded 'café'
2:24 TRUE 'true'
2:30 NULL 'null'
2:34 END_ARRAY ']'
3:2 KEY 'skip'
3:25 KEY 'f'
3:30 NUMBER '-2.5e3', asI64 0 TYPE, asF64 -2500.0
3:36 END_OBJECT '}'
error code 0
3 events, then INVALID: expected a value at column 7
```

- **`JsonReader` is a state machine** with its own stack of the objects and arrays that are
  open, so nesting can't overflow the native stack, and `maxDepth` (1024) limits it with
  `FULL`.
- **`e.text` is the raw text, a view into the input**: a string's or key's without the
  quotes but with any escapes, a number's digits, a bracket, or `true`, `false` or `null`.
  Raw text is enough for comparing keys, which rarely have escapes.
- **`e.decode(strings)` gives the decoded text**: a view into the input when there are no
  escapes, and decoded into `strings` otherwise.
- **`line` and `column` are where the token starts**, counting from 1, the column in bytes.
- **`r.skipValue()` after a `KEY` skips its value**, a whole object or array included; it
  works where an array's element comes too. It fails as `next` does.
- **The loop stops at the end of the input, or at an error.** Afterwards,
  `r.error().code` is 0 if the input was complete. After a failure, `next` keeps failing
  the same way.
- **`skipValue` acts on the reader the loop calls**, which is why the loop is a repeat
  loop that names it.

### JSON Lines

A `.jsonl` file holds one value per line. Each line's buffers are declared in the loop, so
each pass starts with empty ones:

```java
import zink.data.*;
import zink.json.*;

void main() {
    string text = "{\"n\": 1}\n{\"n\": 2}\n{\"n\": oops}\n{\"n\": 4}\n";
    i64 line = 0;
    i64 sum = 0;
    for (string l : text.lines()) {
        line++;
        u8[..>] strings = [];
        Node[..>] tree = [];
        JsonParser p = new JsonParser(l);
        Value v @bad = p.parse(tree, strings);
        if (bad) {
            println("line ${line}: ${p.error().message} at column ${p.error().column}");
            continue;
        }
        sum += v.get("n").asI64Or(0);
    }
    println("sum ${sum}");
}
```

```output
line 3: expected a value at column 7
sum 7
```

### `JsonWriter`

```java
import zink.json.*;
import zink.time.*;

void main() {
    u8[..>] out = [];
    JsonWriter w = new JsonWriter(out);
    w.indent = 2;
    w.beginObject();
    w.key("name");
    w.write("zink \"quoted\"\n");
    w.key("ports");
    w.beginArray();
    w.write(80);
    w.write(443);
    w.end();
    w.key("big");
    w.write((u64) 18446744073709551615);
    w.key("ratio");
    w.write(0.1);
    w.key("third");
    w.write(1.0 / 3.0);
    w.key("ok");
    w.write(true);
    w.key("none");
    w.writeNull();
    w.key("exact");
    w.writeNumber("1.50");
    w.key("when");
    w.write(Timestamp.fromUnix(0).utc());
    w.key("empty");
    w.beginArray();
    w.end();
    w.end();
    u8 err = @w.finish();
    println(out);
    println("finish: ${err}");
}
```

```output
{
  "name": "zink \"quoted\"\n",
  "ports": [
    80,
    443
  ],
  "big": 18446744073709551615,
  "ratio": 0.1,
  "third": 0.3333333333333333,
  "ok": true,
  "none": null,
  "exact": 1.50,
  "when": "1970-01-01T00:00:00Z",
  "empty": []
}
finish: 0
```

```java
JsonWriter w = new JsonWriter(u8[..>]& out)   // appends to out
// every DataWriter method: beginObject, beginArray, end, key, write, writeNull, writeNumber, finish

class JsonWriter { i64 indent = 0; bool asciiOnly = false; ... }
void json.write(u8[..>]& out, Value v, i64 indent)   // a whole tree in one call: TYPE
```

- **`indent`** is spaces per level; 0, the default, writes everything on one line.
- **Strings are escaped as JSON requires**: `"`, `\` and control characters. With
  `asciiOnly`, everything beyond ASCII is escaped too, as `\u` escapes and surrogate pairs.
- **Floats are written in the shortest form that reads back the same** (`0.1`). An `f32`
  is written as the `f64` it converts to, so `(f32) 0.1` is `0.10000000149011612`.
- **`writeNumber` and `write(Value)` keep a number exactly as the input spelled it**, so
  `12345678901234567890` and `1.50` survive reformatting.
- **A `DateTime` is written as an RFC 3339 string**, since JSON has no dates.
- **Only `finish()` sets a status**, the first problem there was:
  - `TYPE` for a value JSON has no form for: a NaN or an infinity, written as `null`, or an
    absent `Value`, written as `null` too;
  - `INVALID` for misuse, or for `writeNumber` of text that isn't a JSON number, which is
    written as `null`.
- **Misuse is written so the output stays well formed**: a value in an object without a
  key gets the key `""`, a key with no value gets `null`, `finish()` closes whatever is
  open, and a `finish()` with nothing written writes `null`. A second value at the top
  level goes on a new line, as JSON Lines has it, and is reported too.
- **It holds a reference to `out`**, which must outlive it, and the bytes are there as soon
  as each call returns.

```java
import zink.json.*;

void main() {
    u8[..>] out = [];
    JsonWriter w = new JsonWriter(out);
    w.beginArray();
    w.write(1.5);
    w.write(0.0 / 0.0);
    w.end();
    u8 st = @w.finish();
    println("${out} -> ${status.name(st)}");

    out.clear();
    JsonWriter m = new JsonWriter(out);
    m.beginObject();
    m.key("ok");
    m.write(1);
    m.write(true);                  // no key
    m.key("waiting");               // no value
    m.key("open");
    m.beginArray();
    u8 st2 = @m.finish();           // closes the array and the object
    println("${out} -> ${status.name(st2)}");

    out.clear();
    JsonWriter a = new JsonWriter(out);
    a.asciiOnly = true;
    a.beginArray();
    a.write("bé😀\u{1}");
    a.writeNumber("0x10");
    a.end();
    u8 st3 = @a.finish();
    println("${out} -> ${status.name(st3)}");

    out.clear();
    JsonWriter lines = new JsonWriter(out);
    lines.write(1);
    lines.write(2);
    u8 st4 = @lines.finish();
    print(out);
    println(" -> ${status.name(st4)}");
}
```

```output
[1.5,null] -> TYPE
{"ok":1,"":true,"waiting":null,"open":[]} -> INVALID
["b\u00e9\ud83d\ude00\u0001",null] -> INVALID
1
2 -> INVALID
```

### Re-indenting and sorting: `examples/jsonfmt.zn`

`examples/jsonfmt.zn` puts the package together. Without `-s` it copies `JsonReader`'s
events straight to a `JsonWriter`, with no tree; with `-s` it parses into a tree and writes
every object with its keys sorted, keeping its own stack instead of recursing.

```
$ cat in.json
{"b": [1, 2.50], "a": {"y": null, "x": "\u00e9"}}
$ zink examples/jsonfmt.zn in.json
{
  "b": [
    1,
    2.50
  ],
  "a": {
    "y": null,
    "x": "é"
  }
}
$ zink examples/jsonfmt.zn -s -c in.json
{"a":{"x":"é","y":null},"b":[1,2.50]}
$ echo '[1, {"a": 2,}]' | zink examples/jsonfmt.zn
jsonfmt: stdin:1:13: expected a key in double quotes
```

## What isn't written yet

These are designed in `STDLIB.md` and not in the library yet; [What isn't there
yet](15-not-yet.md#the-library) lists them with the rest.

- **`zink.toml` and `zink.yaml`**, `TomlParser` and `YamlParser`, which parse into the same
  tree, and `toml.write` and `yaml.write`
  ([STDLIB §9.5](../STDLIB.md#95-toml-zinktoml-later), [§9.6](../STDLIB.md#96-yaml-zinkyaml-later)).
  So `Kind.DATETIME`, `asDateTime()`, `tag()` and `Node.flags` have nothing to report yet.
- **JSON5**, as `JsonParser`'s `json5` field, and the writer's `json5`
  ([STDLIB §9.4](../STDLIB.md#94-json5-later)).
- **`TreeBuilder`**, which builds a tree from program data
  ([STDLIB §9.7](../STDLIB.md#97-writing)).
- **`hash.xxh3`**, which waits for a reference implementation to test against; BLAKE3,
  HMAC, and SHA-1 and MD5 for checking old checksums come later
  ([STDLIB §8.3](../STDLIB.md#83-hash)).
- **Reading into classes**, `cfg.into<Config>()`, which needs compile-time access to a
  class's fields ([STDLIB §11](../STDLIB.md#11-open-questions)).
