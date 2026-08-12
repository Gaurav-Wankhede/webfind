use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use webfind::engine::proxy_pool::{ProxyEndpoint, ProxyPool, RotationStrategy};

fn create_test_proxies(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!("http://proxy{}.example.com:8080", i))
        .collect()
}

fn bench_proxy_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("proxy_selection");

    for pool_size in [10, 100, 1000, 10000].iter() {
        let proxies = create_test_proxies(*pool_size);
        let pool = ProxyPool::from_list(&proxies).unwrap();

        group.throughput(Throughput::Elements(*pool_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(pool_size), &pool, |b, pool| {
            b.iter(|| {
                let proxy = pool.select(None).unwrap();
                black_box(proxy);
            });
        });
    }

    group.finish();
}

fn bench_proxy_selection_with_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("proxy_selection_strategies");

    let proxies = create_test_proxies(1000);

    let strategies = vec![
        ("random", RotationStrategy::Random),
        ("round_robin", RotationStrategy::RoundRobin),
        ("weighted", RotationStrategy::Weighted),
        ("sticky", RotationStrategy::Sticky),
    ];

    for (name, strategy) in strategies {
        let pool = ProxyPool::from_list(&proxies)
            .unwrap()
            .with_strategy(strategy);

        group.bench_with_input(BenchmarkId::from_parameter(name), &pool, |b, pool| {
            b.iter(|| {
                let proxy = pool.select(None).unwrap();
                black_box(proxy);
            });
        });
    }

    group.finish();
}

fn bench_proxy_pool_len(c: &mut Criterion) {
    let mut group = c.benchmark_group("proxy_pool_len");

    let proxies = create_test_proxies(10000);
    let pool = ProxyPool::from_list(&proxies).unwrap();

    group.bench_function("len_10000", |b| {
        b.iter(|| {
            let len = pool.len().unwrap();
            black_box(len);
        });
    });

    group.finish();
}

fn bench_proxy_add(c: &mut Criterion) {
    let mut group = c.benchmark_group("proxy_add");

    let pool = ProxyPool::new();

    group.bench_function("add_1000", |b| {
        b.iter(|| {
            for i in 0..1000 {
                let ep = ProxyEndpoint::from_url(&format!("http://proxy{}.example.com:8080", i))
                    .unwrap();
                pool.add(ep).unwrap();
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_proxy_selection,
    bench_proxy_selection_with_strategies,
    bench_proxy_pool_len,
    bench_proxy_add
);
criterion_main!(benches);
