//! Engine registry: the default engine set and module declarations.

use std::sync::Arc;

use super::Engine;
use super::client::Client;

pub mod arxiv;
pub mod bing;
pub mod crates_io;
pub mod ddg;
pub mod hn;
pub mod lobsters;
pub mod marginalia;
pub mod mdn;
pub mod mojeek;
pub mod stackoverflow;
pub mod wikipedia;

/// The default engine set, in query fan-out order.
///
/// General web engines (duckduckgo, bing, mojeek, marginalia) always return
/// results; the vertical engines (hn, lobsters, arxiv, wikipedia,
/// stackoverflow, crates-io, mdn) return results only when the query matches
/// their domain, and RRF down-weights their absence. All are key-free.
#[must_use]
pub fn default_engines(client: Client) -> Vec<Arc<dyn Engine>> {
    vec![
        Arc::new(ddg::DuckDuckGoEngine::new(client.clone())),
        Arc::new(bing::BingEngine::new(client.clone())),
        Arc::new(mojeek::MojeekEngine::new(client.clone())),
        Arc::new(marginalia::MarginaliaEngine::new(client.clone())),
        Arc::new(hn::HnEngine::new(client.clone())),
        Arc::new(lobsters::LobstersEngine::new(client.clone())),
        Arc::new(arxiv::ArxivEngine::new(client.clone())),
        Arc::new(wikipedia::WikipediaEngine::new(client.clone())),
        Arc::new(stackoverflow::StackOverflowEngine::new(client.clone())),
        Arc::new(crates_io::CratesIoEngine::new(client.clone())),
        Arc::new(mdn::MdnEngine::new(client)),
    ]
}
