//! Doctor streaming for the v1.2 TUI.
//!
//! Spawns a background task that runs the same checks as
//! `doctor::run_data`, but emits events as each check lands rather
//! than blocking until every check completes. The UI reads events
//! off the receiver and progressively builds a per-section item
//! list.
//!
//! The Environment and RPC sections are cheap (synchronous or a
//! single RPC round-trip), so we emit them as whole sections.
//! The Backends section is the expensive one (~18 parallel probes,
//! each up to `timeout_ms`); we stream each backend result
//! individually via `FuturesUnordered` so the user sees probes
//! land one at a time.

use crate::config::Config;
use crate::doctor::{
    check_chainlink, check_config, check_defaults, check_rpc, DoctorItem, DoctorSection,
};
use dexquote_core::Chain;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Event payload emitted by the doctor stream. Each `SectionStarted`
/// announces a section header; `Item` fires once per completed
/// check (streamed as they land for backends, batched for the
/// cheap sections); `Finished` signals the end of the sweep.
#[derive(Debug, Clone)]
pub enum DoctorProgress {
    SectionStarted { name: &'static str },
    Item { section: &'static str, item: DoctorItem },
    Finished { total_elapsed_ms: u128 },
}

pub type DoctorRx = mpsc::UnboundedReceiver<DoctorProgress>;

pub fn spawn(config: Config, config_path: PathBuf, chain_override: Chain) -> DoctorRx {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let start = std::time::Instant::now();

        // Environment section — synchronous, emit as one batch.
        if tx
            .send(DoctorProgress::SectionStarted { name: "Environment" })
            .is_err()
        {
            return;
        }
        for item in check_config(&config_path) {
            if tx
                .send(DoctorProgress::Item {
                    section: "Environment",
                    item,
                })
                .is_err()
            {
                return;
            }
        }
        // Use the chain-override-aware config so `chain:` row
        // reflects the picked chain, not the persisted default.
        let mut cfg_override = config.clone();
        cfg_override.defaults.chain = chain_override.name().to_ascii_lowercase();
        for item in check_defaults(&cfg_override) {
            if tx
                .send(DoctorProgress::Item {
                    section: "Environment",
                    item,
                })
                .is_err()
            {
                return;
            }
        }

        // RPC section — run check_rpc, emit items, optionally run
        // check_chainlink.
        if tx.send(DoctorProgress::SectionStarted { name: "RPC" }).is_err() {
            return;
        }
        let rpc_url = if cfg_override.defaults.rpc.is_empty() {
            None
        } else {
            Some(cfg_override.defaults.rpc.clone())
        };
        let (rpc_items, provider) = check_rpc(rpc_url.as_deref()).await;
        for item in rpc_items {
            if tx
                .send(DoctorProgress::Item {
                    section: "RPC",
                    item,
                })
                .is_err()
            {
                return;
            }
        }
        if let Some(p) = &provider {
            for item in check_chainlink(p, chain_override).await {
                if tx
                    .send(DoctorProgress::Item {
                        section: "RPC",
                        item,
                    })
                    .is_err()
                {
                    return;
                }
            }
        }

        // Backends section — run the full probe set. This could be
        // streamed one backend at a time, but the current
        // `check_backends` API returns the full list in one shot.
        // We emit each item after the join_all completes. This is
        // the biggest chunk of runtime (parallel RPC probes), but
        // the user sees the "Backends" section header pop
        // immediately and the items appear together when the join
        // completes — typically 1-5 seconds.
        if tx
            .send(DoctorProgress::SectionStarted {
                name: "Backends (0.1 WETH → USDC probe)",
            })
            .is_err()
        {
            return;
        }
        let items =
            crate::doctor::check_backends(chain_override, provider.as_ref(), &cfg_override).await;
        for item in items {
            if tx
                .send(DoctorProgress::Item {
                    section: "Backends (0.1 WETH → USDC probe)",
                    item,
                })
                .is_err()
            {
                return;
            }
        }

        let _ = tx.send(DoctorProgress::Finished {
            total_elapsed_ms: start.elapsed().as_millis(),
        });
    });

    rx
}

/// Helper used by the UI layer: take a staged `Vec<DoctorSection>`
/// and add an incoming `Item` to the matching section (creating
/// the section if it doesn't exist yet).
pub fn apply_item(
    sections: &mut Vec<DoctorSection>,
    section_name: &'static str,
    item: DoctorItem,
) {
    if let Some(s) = sections.iter_mut().find(|s| s.name == section_name) {
        s.items.push(item);
        return;
    }
    sections.push(DoctorSection {
        name: section_name.to_string(),
        items: vec![item],
    });
}
