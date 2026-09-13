use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use webfind::engine::fingerprint::FingerprintGenerator;

fn bench_fingerprint_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("fingerprint_generation");

    let generator = FingerprintGenerator::new();

    for batch_size in [1, 10, 100, 1000].iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    for _ in 0..batch_size {
                        let fp = generator.generate_for_tool("test").unwrap();
                        black_box(fp);
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_fingerprint_generation_with_health(c: &mut Criterion) {
    let mut group = c.benchmark_group("fingerprint_generation_with_health");

    let generator = FingerprintGenerator::new();

    // Pre-populate health store with some working IPs
    for _ in 0..100 {
        generator.generate_for_tool("test").unwrap();
    }

    group.bench_function("with_100_working_ips", |b| {
        b.iter(|| {
            let fp = generator.generate_for_tool("test").unwrap();
            black_box(fp);
        });
    });

    group.finish();
}

fn bench_fingerprint_id_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("fingerprint_id_generation");

    let generator = FingerprintGenerator::new();

    group.bench_function("generate_id", |b| {
        b.iter(|| {
            let fp = generator.generate_for_tool("test").unwrap();
            black_box(fp.id);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_fingerprint_generation,
    bench_fingerprint_generation_with_health,
    bench_fingerprint_id_generation
);
criterion_main!(benches);
