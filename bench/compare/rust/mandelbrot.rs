// mandelbrot: the Benchmarks Game Mandelbrot set, an N x N bitmap with up to
// 50 iterations per pixel, packed eight pixels to a byte. It measures a tight
// floating-point loop with an early exit. The program prints a checksum of the
// bitmap instead of the bitmap itself.
mod bench;
use bench::*;

const N: i64 = 1000;                     // BENCH_N
const MAX_ITER: i32 = 50;

fn main() {
    let mut image: Vec<u8> = Vec::with_capacity((N * ((N + 7) / 8)) as usize);
    let inv = 2.0 / N as f64;
    let mut inside: i64 = 0;
    for y in 0..N {
        let ci = y as f64 * inv - 1.0;
        let mut bits: u32 = 0;
        let mut count = 0;
        for x in 0..N {
            let cr = x as f64 * inv - 1.5;
            let (mut zr, mut zi, mut tr, mut ti) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            let mut i = 0;
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
        if count > 0 {
            image.push((bits << (8 - count)) as u8);
        }
    }
    // FNV-1a over the bitmap.
    let mut h: u64 = 14695981039346656037;
    for &b in &image {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    emit(image.len() as i64);
    emit(inside);
    println!("{}", h);
}
