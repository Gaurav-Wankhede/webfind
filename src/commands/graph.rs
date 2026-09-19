use anyhow::Context;

use webfind::cli::GraphStoreArg;
use webfind::engine::crawl_graph::TraversalDirection;

pub async fn run(
    cfg: &webfind::config::WebfindConfig,
    url: String,
    depth: u32,
    direction: webfind::cli::DirectionArg,
    graph_store: Option<GraphStoreArg>,
    turso_path: Option<String>,
) -> anyhow::Result<()> {
    let graph_store = webfind::config::resolve_graph_store(cfg, graph_store);
    let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());
    let store = crate::commands::build_graph_store(
        &graph_store,
        &turso_path,
        cfg.turso.as_ref().and_then(|t| t.encryption_key.as_deref()),
    )
    .await
    .context("connect to graph store")?;

    let direction = match direction {
        webfind::cli::DirectionArg::Inbound => TraversalDirection::Inbound,
        webfind::cli::DirectionArg::Outbound => TraversalDirection::Outbound,
        webfind::cli::DirectionArg::Both => TraversalDirection::Both,
    };
    let visited = webfind::engine::crawl_graph::traverse_graph_with_depth(store, &url, depth, direction).await;

    println!(
        "Graph traversal: {} (max_depth={}, direction={:?}, store={})",
        url,
        depth,
        direction,
        graph_store.as_str()
    );
    println!("Discovered {} URLs:", visited.len());
    for node in &visited {
        if node.depth == 0 {
            println!("  [depth 0] {}", node.url);
        } else {
            let indent = "    ".repeat((node.depth.saturating_sub(1)) as usize);
            println!("  [depth {}] {}└── {}", node.depth, indent, node.url);
        }
    }
    Ok(())
}
