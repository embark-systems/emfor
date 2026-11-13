//! [`PluginSet`]: the plugins of the applied config, and the two-phase
//! change to a new config's plugins (prepare, then commit or roll back).

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::runtime::Handle;

use super::host::PluginHost;
use super::{PluginFactory, plugin_checker_id};
use crate::checker::Registry;
use crate::config::{Config, ConfigError, PluginSpec, key_path};
use crate::live::PluginStatus;

/// Running plugins of the applied config, by name.
#[derive(Debug, Default)]
pub struct PluginSet {
    hosts: BTreeMap<String, Arc<PluginHost>>,
}

/// Plugins for a config being loaded: kept and newly started hosts, and the
/// start failures as config errors.
#[derive(Debug, Default)]
pub struct Prepared {
    hosts: BTreeMap<String, Arc<PluginHost>>,
    started: Vec<Arc<PluginHost>>,
    pub errors: Vec<ConfigError>,
}

impl Prepared {
    /// Built-in checkers plus one factory per running plugin. Build one per
    /// load: factories remember validation timeouts. `applied` is the
    /// running config on a reload: its options for an unchanged plugin are
    /// accepted while that plugin is restarting.
    pub fn registry(&self, handle: Handle, applied: Option<&Config>) -> Registry {
        let mut r = Registry::builtin();
        for host in self.hosts.values() {
            let id = plugin_checker_id(host.name(), host.command());
            let known = applied
                .map(|c| {
                    c.services
                        .iter()
                        .filter(|s| s.checker_id == id)
                        .map(|s| s.check_options.clone())
                        .collect()
                })
                .unwrap_or_default();
            let factory =
                PluginFactory::new(host.clone(), handle.clone()).with_known_options(known);
            r.register(Arc::new(factory));
        }
        r
    }

    /// Stop the plugins started for this load; kept ones keep running.
    pub async fn rollback(self) {
        futures::future::join_all(self.started.iter().map(|h| h.stop())).await;
    }
}

impl PluginSet {
    /// Reuse running plugins whose command is unchanged; start the others,
    /// all at once.
    pub async fn prepare(&self, specs: &[PluginSpec]) -> Prepared {
        let mut prepared = Prepared::default();
        let mut to_start = Vec::new();
        for spec in specs {
            match self.hosts.get(&spec.name) {
                Some(h) if h.command() == spec.command.as_slice() => {
                    prepared.hosts.insert(spec.name.clone(), h.clone());
                }
                _ => to_start.push(spec),
            }
        }
        let results = futures::future::join_all(
            to_start
                .iter()
                .map(|s| PluginHost::start(&s.name, &s.command)),
        )
        .await;
        for (spec, result) in to_start.into_iter().zip(results) {
            match result {
                Ok(h) => {
                    let h = Arc::new(h);
                    prepared.started.push(h.clone());
                    prepared.hosts.insert(spec.name.clone(), h);
                }
                Err(e) => prepared.errors.push(ConfigError {
                    path: key_path(&key_path("plugins", &spec.name), "command"),
                    message: e,
                }),
            }
        }
        prepared
    }

    /// Make `prepared` the applied plugins; stop the ones no longer
    /// declared or replaced by a new command.
    pub async fn commit(&mut self, prepared: Prepared) {
        let old = std::mem::replace(&mut self.hosts, prepared.hosts);
        let dropped: Vec<_> = old
            .into_values()
            .filter(|h| !self.hosts.values().any(|n| Arc::ptr_eq(n, h)))
            .collect();
        futures::future::join_all(dropped.iter().map(|h| h.stop())).await;
    }

    /// Status of each applied plugin, for `/metrics`.
    pub fn statuses(&self) -> Vec<(String, Arc<PluginStatus>)> {
        self.hosts
            .iter()
            .map(|(name, h)| (name.clone(), h.status()))
            .collect()
    }

    /// Stop every plugin.
    pub async fn stop_all(&mut self) {
        let hosts = std::mem::take(&mut self.hosts);
        futures::future::join_all(hosts.values().map(|h| h.stop())).await;
    }
}
