// mandelbrot: the Benchmarks Game Mandelbrot set, an N x N bitmap with up to
// 50 iterations per pixel, packed eight pixels to a byte. It measures a tight
// floating-point loop with an early exit. The program prints a checksum of the
// bitmap instead of the bitmap itself. Unlike bench/plb/mandelbrot, which works
// on 32 pixels at a time for the vector unit, this is the plain per-pixel loop
// the other three languages use.

let N = 1000;                            // BENCH_N
let MAX_ITER = 50;

fn main() {
    var image: u8[>..] = [];
    let inv = 2.0 / N;
    var inside = 0;
    for y in N {
        let ci = y * inv - 1.0;
        var bits = 0;
        var count = 0;
        for x in N {
            let cr = x * inv - 1.5;
            var zr = 0.0;
            var zi = 0.0;
            var tr = 0.0;
            var ti = 0.0;
            var i = 0;
            while i < MAX_ITER && tr + ti <= 4.0 {
                zi = 2.0 * zr * zi + ci;
                zr = tr - ti + cr;
                tr = zr * zr;
                ti = zi * zi;
                i += 1;
            }
            bits <<= 1;
            if tr + ti <= 4.0 {
                bits |= 1;
                inside += 1;
            }
            count += 1;
            if count == 8 {
                image.push(bits as u8);
                bits = 0;
                count = 0;
            }
        }
        if count > 0 { image.push((bits << (8 - count)) as u8); }
    }
    // FNV-1a over the bitmap.
    var h: u64 = 14695981039346656037;
    for b in image { h = (h ^ b) * 1099511628211; }
    print(image.len);
    print(inside);
    print(h);
}
