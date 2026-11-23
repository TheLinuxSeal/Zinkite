// fannkuch-redux (Benchmarks Game): for every permutation of 0..n-1, count the
// prefix reversals ("pancake flips") until 0 comes first. Prints the checksum
// and the largest flip count. Small arrays, tight loops, indexing and swaps.
// The plain single-threaded algorithm, the same in all four languages (Goose's
// own bench/plb/fannkuch-redux/1.goose is a bit-packed version of it).

import std;

let N = 10;                              // BENCH_N

fn main() {
    let n = N;
    var perm: i64[16] = [0; 16];
    var perm1: i64[16] = [0; 16];
    var count: i64[16] = [0; 16];
    for i in n { perm1[i] = i; }
    var max_flips = 0;
    var perm_count = 0;
    var checksum = 0;
    var r = n;
    while true {
        while r != 1 {
            count[r - 1] = r;
            r--;
        }
        perm = perm1;
        var flips = 0;
        var k = perm[0];
        while k != 0 {
            reverse(perm[..k + 1]);
            flips++;
            k = perm[0];
        }
        max_flips = max(max_flips, flips);
        checksum += if perm_count % 2 == 0 { flips } else { -flips };
        // The next permutation: rotate the first r + 1 elements left by one,
        // and each counter that runs out brings one more element in.
        while true {
            if r == n {
                print(checksum);
                print(max_flips);
                return;
            }
            let perm0 = perm1[0];
            for i in r { perm1[i] = perm1[i + 1]; }
            perm1[r] = perm0;
            count[r]--;
            if count[r] > 0 { break; }
            r++;
        }
        perm_count++;
    }
}
