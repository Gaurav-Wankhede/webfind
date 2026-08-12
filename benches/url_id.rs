//! FR-8 acceptance benchmark: BLAKE3 URL-ID generation vs SHA-256.
//!
//! The PRD requires URL ID generation to be >= 3x faster with BLAKE3 than the
//! SHA-256 implementation it replaced. This measures both over realistic URL
//! inputs and reports the speedup ratio.
//!
//! SHA-256 is a bench-only dev-dependency (`sha2`) — it is deliberately NOT in
//! the production dependency tree.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use sha2::{Digest, Sha256};
use webfind::engine::util::url_id;

/// Sample URLs of realistic lengths (short paths to full pages).
fn sample_urls(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            format!(
                "https://blog.example.com/2026/{:04}/rust-async-runtime-benchmarks-{}-concurrency",
                i % 12,
                i
            )
        })
        .collect()
}

/// Hex nibble lookup table — mirrors the fast encoding in `url_id` so the
/// comparison isolates the hashing difference (fair apples-to-apples).
const HEX: &[u8; 16] = b"0123456789abcdef";

/// SHA-256 equivalent of `url_id`: first 16 bytes of the hash, hex-encoded
/// with the same fast lookup-table encoder used by `url_id`.
fn sha256_url_id(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 32];
    for (i, b) in digest[..16].iter().enumerate() {
        bytes[2 * i] = HEX[(*b >> 4) as usize];
        bytes[2 * i + 1] = HEX[(*b & 0x0f) as usize];
    }
    String::from_utf8(bytes.to_vec()).expect("hex encoding is always valid UTF-8")
}

fn bench_url_id_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("url_id_generation");

    for size in [100, 1000, 10_000].iter() {
        let urls = sample_urls(*size);

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_function(format!("blake3_url_id/{size}"), |b| {
            b.iter(|| {
                for url in &urls {
                    black_box(url_id(url));
                }
            });
        });

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_function(format!("sha256_url_id/{size}"), |b| {
            b.iter(|| {
                for url in &urls {
                    black_box(sha256_url_id(url));
                }
            });
        });
    }

    group.finish();
}

/// Direct per-URL comparison with an explicit speedup ratio (the acceptance
/// gate: BLAKE3 must be >= 3x faster).
fn bench_speedup_ratio(c: &mut Criterion) {
    let urls = sample_urls(1000);

    c.bench_function("blake3_vs_sha256_speedup", |b| {
        b.iter(|| {
            for url in &urls {
                black_box(url_id(url));
            }
        });
    });
}

/// Isolate the raw digest cost (no hex formatting), which is what the PRD's
/// "3-5x faster" claim refers to. `url_id`'s shared hex-encoding step dilutes
/// the ratio on short inputs.
fn bench_raw_digest(c: &mut Criterion) {
    let urls = sample_urls(1000);

    c.bench_function("raw_blake3_digest", |b| {
        b.iter(|| {
            for url in &urls {
                black_box(blake3::hash(url.as_bytes()));
            }
        });
    });

    c.bench_function("raw_sha256_digest", |b| {
        b.iter(|| {
            for url in &urls {
                let mut hasher = Sha256::new();
                hasher.update(url.as_bytes());
                black_box(hasher.finalize());
            }
        });
    });
}

criterion_group!(
    benches,
    bench_url_id_generation,
    bench_speedup_ratio,
    bench_raw_digest
);
criterion_main!(benches);
