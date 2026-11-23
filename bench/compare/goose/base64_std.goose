// base64: encode N random bytes (RFC 4648, with padding) and decode the text
// back, each 16 times into a new buffer, with the standard library's codec.
// Prints the text's length and FNV-1a hash, a sum over every pass, and whether
// the round trip gave the input back. After Goose's bench/kostya2/base64.

import std;
import rng;
import base64;

let N = 1000000;                         // BENCH_N
let TRIES = 16;

fn main() {
    var input: u8[>..] = [];
    var r: u64 = 12345;
    for i in N {
        r = xs_next(r);
        input.push(xs_mod(r, 256) as! u8);
    }
    var text: u8[>..] = [];
    base64::encode(text, input);
    var h: u64 = 14695981039346656037;
    for b in text { h = (h ^ b) * 1099511628211; }

    // Each pass's buffer is new, and gone again at its closing brace. One byte of
    // each result goes into the sum, so no pass can be skipped.
    var encoded = 0;
    for t in TRIES {
        var e: u8[>..] = [];
        base64::encode(e, input);
        encoded += e.len + e[t * 1000003 % e.len];
    }
    var decoded = 0;
    for t in TRIES {
        var d: u8[>..] = [];
        if !base64::decode(d, text) { abort("invalid base64"); }
        decoded += d.len + d[t * 1000003 % d.len];
    }
    var back: u8[>..] = [];
    if !base64::decode(back, text) { abort("invalid base64"); }

    print(text.len);
    print(h);
    print(encoded);
    print(decoded);
    print(if back == input { 1 } else { 0 });
}
