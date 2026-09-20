// webfind-trainer: Master 8-Pillar Prominent Seed Registry.
// Defines official primary seed targets across all critical technology domains.

pub struct ProminentPillar {
    pub name: &'static str,
    pub seeds: &'static [&'static str],
}

pub const MASTER_PILLARS: &[ProminentPillar] = &[
    // 1. Web Frameworks & Modern Runtimes
    ProminentPillar {
        name: "Web Frameworks & Runtimes",
        seeds: &[
            "https://nextjs.org/docs",
            "https://remix.run/docs",
            "https://astro.build/",
            "https://svelte.dev/docs",
            "https://vuejs.org/guide/",
            "https://react.dev/reference/react",
            "https://bun.sh/docs",
            "https://deno.com/manual",
            "https://actix.rs/docs/",
        ],
    },
    // 2. Package & Specification Registries
    ProminentPillar {
        name: "Package Registries",
        seeds: &[
            "https://crates.io/",
            "https://docs.rs/",
            "https://pypi.org/",
            "https://pkg.go.dev/",
            "https://jsr.io/",
            "https://www.npmjs.com/",
        ],
    },
    // 3. Language Official Specs & References
    ProminentPillar {
        name: "Language Specs & Manuals",
        seeds: &[
            "https://doc.rust-lang.org/book/",
            "https://doc.rust-lang.org/reference/",
            "https://doc.rust-lang.org/nomicon/",
            "https://docs.python.org/3/",
            "https://go.dev/ref/spec",
            "https://www.typescriptlang.org/docs/",
            "https://en.cppreference.com/w/",
        ],
    },
    // 4. Web Standards & Browser Engines
    ProminentPillar {
        name: "Web Standards",
        seeds: &[
            "https://developer.mozilla.org/en-US/docs/Web",
            "https://html.spec.whatwg.org/",
            "https://www.w3.org/TR/",
            "https://tc39.es/",
            "https://web.dev/",
        ],
    },
    // 5. Deep Learning, AI & Scientific
    ProminentPillar {
        name: "Deep Learning & AI",
        seeds: &[
            "https://burn.dev/",
            "https://huggingface.co/docs",
            "https://pytorch.org/docs/stable/",
            "https://paperswithcode.com/",
            "https://vllm.ai/",
            "https://ollama.com/",
        ],
    },
    // 6. Cloud, Infrastructure & Edge
    ProminentPillar {
        name: "Cloud & Infrastructure",
        seeds: &[
            "https://developers.cloudflare.com/",
            "https://kubernetes.io/docs/home/",
            "https://docs.docker.com/",
            "https://www.kernel.org/doc/html/latest/",
            "https://wiki.archlinux.org/",
        ],
    },
    // 7. High-Signal Communities & Reddit
    ProminentPillar {
        name: "Communities & Reddit",
        seeds: &[
            "https://www.reddit.com/r/rust/",
            "https://www.reddit.com/r/programming/",
            "https://www.reddit.com/r/MachineLearning/",
            "https://www.reddit.com/r/LocalLLaMA/",
            "https://news.ycombinator.com/",
        ],
    },
    // 8. Engineering News & Deep Tech Blogs
    ProminentPillar {
        name: "Engineering Blogs & News",
        seeds: &[
            "https://arstechnica.com/",
            "https://www.infoq.com/",
            "https://lwn.net/",
            "https://semianalysis.com/",
            "https://blog.cloudflare.com/",
        ],
    },
    // 9. Security Standards & Protocol RFCs
    ProminentPillar {
        name: "Security Standards & RFCs",
        seeds: &[
            "https://owasp.org/www-project-top-ten/",
            "https://owasp.org/www-project-api-security/",
            "https://www.nist.gov/",
            "https://csrc.nist.gov/publications/sp800",
            "https://www.cisa.gov/resources-tools",
            "https://www.rfc-editor.org/rfc/rfc9110.html",
            "https://slsa.dev/spec/v1.0/",
        ],
    },
    // 10. AI Agent Protocols & Machine API Contracts
    ProminentPillar {
        name: "AI Protocols & API Contracts",
        seeds: &[
            "https://modelcontextprotocol.io/",
            "https://spec.openapis.org/oas/v3.1.0",
            "https://spec.graphql.org/draft/",
            "https://www.asyncapi.com/",
            "https://grpc.io/docs/what-is-grpc/introduction/",
            "https://json-schema.org/draft/2020-12/schema",
        ],
    },
    // 11. Distributed & Embedded Databases
    ProminentPillar {
        name: "Distributed & Embedded Databases",
        seeds: &[
            "https://sqlite.org/arch.html",
            "https://sqlite.org/wal.html",
            "https://www.postgresql.org/docs/current/wal-intro.html",
            "https://docs.turso.tech/introduction",
            "https://tikv.org/docs/deep-dive/architecture/overview/",
            "https://rocksdb.org/docs/getting-started.html",
        ],
    },
    // 12. Systems Kernel, Virtualization & Container Runtimes
    ProminentPillar {
        name: "Kernel, Virtualization & Container Runtimes",
        seeds: &[
            "https://www.w3.org/TR/wasm-core-2/",
            "https://ebpf.io/what-is-ebpf/",
            "https://kubernetes.io/docs/concepts/containers/cri/",
            "https://github.com/opencontainers/runtime-spec",
            "https://docs.kernel.org/",
            "https://firecracker-microvm.github.io/",
        ],
    },
    // 13. Compilers, IRs & Intermediate Dialects
    ProminentPillar {
        name: "Compilers, IRs & Intermediate Dialects",
        seeds: &[
            "https://llvm.org/docs/LangRef.html",
            "https://mlir.llvm.org/",
            "https://cranelift.readthedocs.io/",
            "https://gcc.gnu.org/onlinedocs/",
            "https://registry.khronos.org/SPIR-V/specs/unified1/SPIRV.html",
        ],
    },
];
