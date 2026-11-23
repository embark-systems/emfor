//! The audit lines of a reload: what differs between the running config and
//! the one a reload applies, logged one `info` line per difference. The field
//! names and the values of `change` are stable; the wording of a line is
//! not. Nothing here reads or prints a `check_options` value or the content
//! of a secret file.

use std::collections::BTreeMap;

use crate::config::{Backend, Config, PeersConfig, ServiceConfig};

/// Most difference lines of one reload; the rest become one `change=more`.
pub const MAX_AUDIT_LINES: usize = 50;

/// What `-` stands for in a `from` or `to` that has no value.
const NONE: &str = "-";

/// One difference between two configs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    ServiceAdded {
        service: String,
        backends: usize,
    },
    ServiceRemoved {
        service: String,
        backends: usize,
    },
    BackendAdded {
        service: String,
        backend: String,
    },
    BackendRemoved {
        service: String,
        backend: String,
    },
    BackendChanged {
        service: String,
        backend: String,
        field: &'static str,
        from: String,
        to: String,
    },
    ServiceChanged {
        service: String,
        field: &'static str,
        from: String,
        to: String,
    },
    CheckDefinitionChanged {
        service: String,
        digest_from: String,
        digest_to: String,
        /// The backends of the service start over from `unknown`.
        resets_state: bool,
    },
    PeerMemberAdded {
        peer: String,
    },
    PeerMemberRemoved {
        peer: String,
    },
    PeersChanged {
        field: &'static str,
        from: String,
        to: String,
        /// `now`, or `next_start` for a setting read only at start.
        applies: &'static str,
    },
    /// The differences left out after [`MAX_AUDIT_LINES`].
    More {
        omitted: usize,
    },
}

/// The differences from `old` to `new`, in the order they are logged:
/// services by name (a service's own lines before its backends'), then the
/// peer group.
pub fn changes(old: &Config, new: &Config) -> Vec<Change> {
    let (old_services, new_services) = (by_name(old), by_name(new));
    let mut names: Vec<&str> = old_services
        .keys()
        .chain(new_services.keys())
        .copied()
        .collect();
    names.sort();
    names.dedup();

    let mut out = Vec::new();
    for name in names {
        match (old_services.get(name), new_services.get(name)) {
            (None, Some(s)) => out.push(Change::ServiceAdded {
                service: name.to_string(),
                backends: s.backends.len(),
            }),
            (Some(s), None) => out.push(Change::ServiceRemoved {
                service: name.to_string(),
                backends: s.backends.len(),
            }),
            (Some(a), Some(b)) => service_changes(a, b, &mut out),
            (None, None) => {}
        }
    }
    if let (Some(a), Some(b)) = (&old.peers, &new.peers) {
        peer_changes(a, b, &mut out);
    }
    out
}

fn by_name(c: &Config) -> BTreeMap<&str, &ServiceConfig> {
    c.services.iter().map(|s| (s.name.as_str(), s)).collect()
}

fn by_id(s: &ServiceConfig) -> BTreeMap<&str, &Backend> {
    s.backends.iter().map(|b| (b.id.as_str(), b)).collect()
}

fn service_changes(old: &ServiceConfig, new: &ServiceConfig, out: &mut Vec<Change>) {
    let service = &old.name;
    let mut field = |name: &'static str, from: String, to: String| {
        if from != to {
            out.push(Change::ServiceChanged {
                service: service.clone(),
                field: name,
                from,
                to,
            });
        }
    };
    let (a, b) = (&old.settings, &new.settings);
    field("check", old.check.clone(), new.check.clone());
    field(
        "interval",
        format!("{:?}", a.interval),
        format!("{:?}", b.interval),
    );
    field(
        "timeout",
        format!("{:?}", a.timeout),
        format!("{:?}", b.timeout),
    );
    field("rise", a.rise.to_string(), b.rise.to_string());
    field("fall", a.fall.to_string(), b.fall.to_string());
    field("jitter", a.jitter.to_string(), b.jitter.to_string());
    field(
        "on_all_down",
        a.on_all_down.as_str().into(),
        b.on_all_down.as_str().into(),
    );
    field(
        "balance",
        a.balance.as_str().into(),
        b.balance.as_str().into(),
    );
    field("peers", a.peers.to_string(), b.peers.to_string());
    field(
        "slow_start",
        format!("{:?}", a.slow_start),
        format!("{:?}", b.slow_start),
    );
    let ttl = |t: Option<u32>| t.map_or_else(|| NONE.to_string(), |t| t.to_string());
    field("dns_ttl", ttl(old.dns_ttl), ttl(new.dns_ttl));
    if old.digest != new.digest {
        out.push(Change::CheckDefinitionChanged {
            service: service.clone(),
            digest_from: old.digest.clone(),
            digest_to: new.digest.clone(),
            resets_state: old.check_changed(new),
        });
    }

    let (old_backends, new_backends) = (by_id(old), by_id(new));
    let mut ids: Vec<&str> = old_backends
        .keys()
        .chain(new_backends.keys())
        .copied()
        .collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        match (old_backends.get(id), new_backends.get(id)) {
            (None, Some(_)) => out.push(Change::BackendAdded {
                service: service.clone(),
                backend: id.to_string(),
            }),
            (Some(_), None) => out.push(Change::BackendRemoved {
                service: service.clone(),
                backend: id.to_string(),
            }),
            (Some(a), Some(b)) => {
                let site = |s: &Option<String>| s.clone().unwrap_or_else(|| NONE.into());
                let pairs = [
                    ("weight", a.weight.to_string(), b.weight.to_string()),
                    ("site", site(&a.site), site(&b.site)),
                    ("backup", a.backup.to_string(), b.backup.to_string()),
                    ("drain", a.drain.to_string(), b.drain.to_string()),
                ];
                for (field, from, to) in pairs {
                    if from != to {
                        out.push(Change::BackendChanged {
                            service: service.clone(),
                            backend: id.to_string(),
                            field,
                            from,
                            to,
                        });
                    }
                }
            }
            (None, None) => {}
        }
    }
}

fn peer_changes(old: &PeersConfig, new: &PeersConfig, out: &mut Vec<Change>) {
    for m in &old.members {
        if !new.members.contains(m) {
            out.push(Change::PeerMemberRemoved {
                peer: m.to_string(),
            });
        }
    }
    for m in &new.members {
        if !old.members.contains(m) {
            out.push(Change::PeerMemberAdded {
                peer: m.to_string(),
            });
        }
    }
    let path = |p: &Option<std::path::PathBuf>| {
        p.as_ref()
            .map_or_else(|| NONE.to_string(), |p| p.display().to_string())
    };
    let pairs = [
        (
            "encrypt",
            old.encrypt.to_string(),
            new.encrypt.to_string(),
            "now",
        ),
        (
            "accept_plaintext",
            old.accept_plaintext.to_string(),
            new.accept_plaintext.to_string(),
            "now",
        ),
        (
            "cipher",
            old.cipher.as_str().to_string(),
            new.cipher.as_str().to_string(),
            "now",
        ),
        (
            "secret_file",
            old.secret_file.display().to_string(),
            new.secret_file.display().to_string(),
            "now",
        ),
        (
            "previous_secret_file",
            path(&old.previous_secret_file),
            path(&new.previous_secret_file),
            "next_start",
        ),
    ];
    for (field, from, to, applies) in pairs {
        if from != to {
            out.push(Change::PeersChanged {
                field,
                from,
                to,
                applies,
            });
        }
    }
}

/// The first [`MAX_AUDIT_LINES`] changes, then one [`Change::More`] for the
/// rest.
pub fn capped(mut changes: Vec<Change>) -> Vec<Change> {
    if changes.len() > MAX_AUDIT_LINES {
        let omitted = changes.len() - MAX_AUDIT_LINES;
        changes.truncate(MAX_AUDIT_LINES);
        changes.push(Change::More { omitted });
    }
    changes
}

/// Log each change as one `info` line, at most [`MAX_AUDIT_LINES`].
pub fn log_changes(changes: Vec<Change>) {
    for c in capped(changes) {
        log_one(&c);
    }
}

fn log_one(c: &Change) {
    match c {
        Change::ServiceAdded { service, backends } => tracing::info!(
            change = %"service_added", service = %service, backends = *backends,
            "reload: service {service} added with {backends} backends"
        ),
        Change::ServiceRemoved { service, backends } => tracing::info!(
            change = %"service_removed", service = %service, backends = *backends,
            "reload: service {service} removed (had {backends} backends)"
        ),
        Change::BackendAdded { service, backend } => tracing::info!(
            change = %"backend_added", service = %service, backend = %backend,
            "reload: backend {backend} added to {service}"
        ),
        Change::BackendRemoved { service, backend } => tracing::info!(
            change = %"backend_removed", service = %service, backend = %backend,
            "reload: backend {backend} removed from {service}"
        ),
        Change::BackendChanged {
            service,
            backend,
            field,
            from,
            to,
        } => tracing::info!(
            change = %"backend_changed", service = %service, backend = %backend,
            field = %field, from = %from, to = %to,
            "reload: {field} of backend {backend} of {service} was {from}, now {to}"
        ),
        Change::ServiceChanged {
            service,
            field,
            from,
            to,
        } => tracing::info!(
            change = %"service_changed", service = %service, field = %field,
            from = %from, to = %to,
            "reload: {field} of {service} was {from}, now {to}"
        ),
        Change::CheckDefinitionChanged {
            service,
            digest_from,
            digest_to,
            resets_state,
        } => {
            tracing::info!(
                change = %"check_definition_changed", service = %service,
                digest_from = %digest_from, digest_to = %digest_to,
                resets_state = *resets_state,
                "reload: check definition of {service} changed{}",
                if *resets_state { "; its backends start over from unknown" } else { "" }
            )
        }
        Change::PeerMemberAdded { peer } => tracing::info!(
            change = %"peer_member_added", peer = %peer, "reload: peer {peer} added"
        ),
        Change::PeerMemberRemoved { peer } => tracing::info!(
            change = %"peer_member_removed", peer = %peer, "reload: peer {peer} removed"
        ),
        Change::PeersChanged {
            field,
            from,
            to,
            applies,
        } => tracing::info!(
            change = %"peers_changed", field = %field, from = %from, to = %to,
            applies = %applies,
            "reload: [peers] {field} was {from}, now {to}"
        ),
        Change::More { omitted } => tracing::info!(
            change = %"more",
            omitted = *omitted,
            "reload: and {omitted} more changes"
        ),
    }
}

#[cfg(test)]
mod tests;
