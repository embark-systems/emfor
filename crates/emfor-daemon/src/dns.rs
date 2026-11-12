//! DNS server: answers A and AAAA for services under the zone from live
//! health state. All behavior is in the pure function [`answer`]; the
//! hickory-server adapter only decodes and encodes.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use hickory_proto::op::{Edns, MessageType, Metadata, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, SOA, SRV};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use hickory_proto::serialize::binary::BinEncoder;
use hickory_server::net::runtime::Time;
use hickory_server::net::xfer::Protocol;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::{MessageResponseBuilder, Queries};
use tokio::sync::watch;

use crate::config::in_zone;
use crate::live::{LiveService, LiveState, LiveView};
use crate::select::{Family, Selection, select, srv_targets};

/// Settings that reload may change without touching sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsSettings {
    /// Normalized, without trailing dot.
    pub zone: String,
    /// `[dns] ttl`, seconds.
    pub ttl: u32,
    /// SOA serial: daemon start time in Unix seconds, truncated to 32 bits.
    pub serial: u32,
}

/// What to send back. Records carry the query name as received.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub rcode: ResponseCode,
    pub authoritative: bool,
    pub answers: Vec<Record>,
    /// Zone SOA for the authority section (NODATA and NXDOMAIN).
    pub soa: Option<Record>,
    /// A and AAAA records of SRV targets, for the additional section.
    pub additionals: Vec<Record>,
}

impl Reply {
    fn error(rcode: ResponseCode) -> Reply {
        Reply {
            rcode,
            authoritative: false,
            answers: Vec::new(),
            soa: None,
            additionals: Vec::new(),
        }
    }

    fn authoritative(rcode: ResponseCode, answers: Vec<Record>, soa: Option<Record>) -> Reply {
        Reply {
            rcode,
            authoritative: true,
            answers,
            soa,
            additionals: Vec::new(),
        }
    }
}

/// Lowercase, no trailing dot.
fn normalize(name: &Name) -> String {
    let s = name.to_ascii().to_ascii_lowercase();
    s.strip_suffix('.').map(str::to_string).unwrap_or(s)
}

fn fqdn(name: &str) -> Name {
    let mut n = Name::from_ascii(name).expect("normalized name is valid");
    n.set_fqdn(true);
    n
}

/// `<zone>. <ttl> IN SOA ns.<zone>. hostmaster.<zone>. <serial> 3600 600 86400 <ttl>`
pub fn soa_record(settings: &DnsSettings) -> Record {
    let zone = &settings.zone;
    let soa = SOA::new(
        fqdn(&format!("ns.{zone}")),
        fqdn(&format!("hostmaster.{zone}")),
        settings.serial,
        3600,
        600,
        86400,
        settings.ttl,
    );
    Record::from_rdata(fqdn(zone), settings.ttl, RData::SOA(soa))
}

/// First label of an SRV target name: IPv4 dotted with `-`, IPv6 as eight
/// four-digit lowercase groups joined by `-` (never starts or ends with `-`).
pub fn ip_label(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string().replace('.', "-"),
        IpAddr::V6(v6) => v6
            .segments()
            .iter()
            .map(|g| format!("{g:04x}"))
            .collect::<Vec<_>>()
            .join("-"),
    }
}

fn addr_rdata(ip: IpAddr) -> RData {
    match ip {
        IpAddr::V4(v4) => RData::A(A(v4)),
        IpAddr::V6(v6) => RData::AAAA(AAAA(v6)),
    }
}

/// SRV records for service `svc_name`, owned by the query name, with the
/// target addresses as additional records.
fn srv_reply(
    qname: &Name,
    svc_name: &str,
    svc: &LiveService,
    view: &LiveView,
    ttl: u32,
    soa: Record,
) -> Reply {
    let list = match srv_targets(&svc.record, &svc.round_robin.srv, view.site.as_deref()) {
        Ok(l) => l,
        Err(Selection::AllDown) => return Reply::error(ResponseCode::ServFail),
        Err(_) => return Reply::authoritative(ResponseCode::NoError, Vec::new(), Some(soa)),
    };
    let mut answers = Vec::with_capacity(list.len());
    let mut additionals: Vec<Record> = Vec::new();
    let mut seen: Vec<IpAddr> = Vec::new();
    for t in list {
        // A long service name plus a 39-character IPv6 label can exceed the
        // 255-byte name limit; such a target cannot be named at all.
        let Ok(mut target) = Name::from_ascii(format!("{}.{svc_name}.", ip_label(t.ip))) else {
            tracing::debug!("SRV target name for {} under {svc_name} is too long", t.ip);
            return Reply::error(ResponseCode::ServFail);
        };
        target.set_fqdn(true);
        let srv = SRV::new(t.priority, t.weight, t.port, target.clone());
        answers.push(Record::from_rdata(qname.clone(), ttl, RData::SRV(srv)));
        if !seen.contains(&t.ip) {
            seen.push(t.ip);
            additionals.push(Record::from_rdata(target, ttl, addr_rdata(t.ip)));
        }
    }
    let mut r = Reply::authoritative(ResponseCode::NoError, answers, None);
    r.additionals = additionals;
    r
}

/// Compute the reply to one request.
pub fn answer(
    op_code: OpCode,
    questions: &[(Name, RecordType)],
    view: &LiveView,
    settings: &DnsSettings,
) -> Reply {
    if op_code != OpCode::Query {
        return Reply::error(ResponseCode::NotImp);
    }
    let [(qname, qtype)] = questions else {
        return Reply::error(ResponseCode::FormErr);
    };
    let (qtype, name) = (*qtype, normalize(qname));
    let zone = settings.zone.as_str();
    if !in_zone(&name, zone) {
        return Reply::error(ResponseCode::Refused);
    }
    if !view.ready {
        // Starting up: which names exist is not known yet. SERVFAIL is not
        // cached like NXDOMAIN would be.
        return Reply::error(ResponseCode::ServFail);
    }
    let soa = soa_record(settings);
    let nodata = |soa: Record| Reply::authoritative(ResponseCode::NoError, Vec::new(), Some(soa));

    if name == zone && qtype == RecordType::SOA {
        return Reply::authoritative(ResponseCode::NoError, vec![soa], None);
    }
    if let Some(svc) = view.services.get(&name) {
        let ttl = svc.dns_ttl.unwrap_or(settings.ttl);
        let family = match qtype {
            RecordType::A => Family::V4,
            RecordType::AAAA => Family::V6,
            RecordType::SRV => return srv_reply(qname, &name, svc, view, ttl, soa),
            _ => return nodata(soa),
        };
        let counter = svc.round_robin.for_family(family);
        return match select(&svc.record, family, counter, view.site.as_deref()) {
            Selection::Chosen(ip) => {
                let rec = Record::from_rdata(qname.clone(), ttl, addr_rdata(ip));
                Reply::authoritative(ResponseCode::NoError, vec![rec], None)
            }
            Selection::NoFamily => nodata(soa),
            Selection::AllDown => Reply::error(ResponseCode::ServFail),
        };
    }
    if let Some((first, rest)) = name.split_once('.') {
        // `_<label>._tcp.<service>`: SRV owner name.
        if first.len() > 1
            && first.starts_with('_')
            && let Some((svc_name, svc)) = rest
                .strip_prefix("_tcp.")
                .and_then(|s| view.services.get_key_value(s))
        {
            return match qtype {
                RecordType::SRV => {
                    let ttl = svc.dns_ttl.unwrap_or(settings.ttl);
                    srv_reply(qname, svc_name, svc, view, ttl, soa)
                }
                _ => nodata(soa),
            };
        }
        if let Some(svc) = view.services.get(rest) {
            // `_tcp.<service>`: empty non-terminal.
            if first == "_tcp" {
                return nodata(soa);
            }
            // `<ip label>.<service>`: SRV target, any health state.
            let target = svc
                .record
                .backends
                .iter()
                .filter_map(|b| b.ip.parse::<IpAddr>().ok())
                .find(|ip| ip_label(*ip) == first);
            if let Some(ip) = target {
                let wanted = match ip {
                    IpAddr::V4(_) => RecordType::A,
                    IpAddr::V6(_) => RecordType::AAAA,
                };
                if qtype != wanted {
                    return nodata(soa);
                }
                let ttl = svc.dns_ttl.unwrap_or(settings.ttl);
                let rec = Record::from_rdata(qname.clone(), ttl, addr_rdata(ip));
                return Reply::authoritative(ResponseCode::NoError, vec![rec], None);
            }
        }
    }
    let empty_non_terminal = view
        .services
        .keys()
        .any(|s| s != &name && in_zone(s, &name));
    if name == zone || empty_non_terminal {
        return nodata(soa);
    }
    Reply::authoritative(ResponseCode::NXDomain, Vec::new(), Some(soa))
}

/// Largest UDP response: the request's EDNS payload size clamped to
/// 512..=1232, or 512 without EDNS.
pub fn udp_limit(edns_payload: Option<u16>) -> u16 {
    edns_payload.map_or(512, |p| p.clamp(512, 1232))
}

/// Make the response fit `limit` bytes: first drop every additional record
/// (missing glue never sets TC), then drop answers from the end until it
/// fits. Returns true when answers were dropped, so the caller sets TC.
/// Encodes the response the way hickory-server does (same encoder, same
/// sections). Trimming answers here, not in hickory, keeps room for the
/// OPT record, which hickory leaves out when it cuts answers itself.
pub fn fit_response(reply: &mut Reply, queries: &Queries, edns: Option<&Edns>, limit: u16) -> bool {
    // One answer and no additional records (A, AAAA, SOA, negative and
    // error replies) is always far below 512 bytes: skip the extra encode.
    if reply.additionals.is_empty() && reply.answers.len() <= 1 {
        return false;
    }
    let limit = usize::from(limit);
    if encoded_len(reply, queries, edns) <= limit {
        return false;
    }
    reply.additionals.clear();
    if encoded_len(reply, queries, edns) <= limit {
        return false;
    }
    // Largest prefix of the answers that fits, by binary search: `lo` fits
    // (zero answers always does), `hi` does not.
    let all = std::mem::take(&mut reply.answers);
    let (mut lo, mut hi) = (0, all.len());
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        reply.answers = all[..mid].to_vec();
        if encoded_len(reply, queries, edns) <= limit {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    reply.answers = all[..lo].to_vec();
    true
}

/// Wire length of the full response, without a size limit, encoded by
/// hickory-server itself (it does not compress against the question name,
/// so an approximation through `Message` would be 13 bytes short).
fn encoded_len(reply: &Reply, queries: &Queries, edns: Option<&Edns>) -> usize {
    let mut builder = MessageResponseBuilder::new(queries, edns);
    if let Some(e) = edns {
        builder.edns(e);
    }
    let response = builder.build(
        Metadata::new(0, MessageType::Response, OpCode::Query),
        reply.answers.iter(),
        std::iter::empty(),
        reply.soa.iter(),
        reply.additionals.iter(),
    );
    let mut bytes = Vec::with_capacity(512);
    let mut encoder = BinEncoder::new(&mut bytes);
    // A message that cannot be encoded is never sent as is.
    match response.destructive_emit(&mut encoder) {
        Ok(_) => bytes.len(),
        Err(_) => usize::MAX,
    }
}

/// hickory-server adapter around [`answer`].
pub struct Handler {
    live: LiveState,
    settings: watch::Receiver<DnsSettings>,
}

impl Handler {
    pub fn new(live: LiveState, settings: watch::Receiver<DnsSettings>) -> Handler {
        Handler { live, settings }
    }
}

#[async_trait::async_trait]
impl RequestHandler for Handler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let questions: Vec<(Name, RecordType)> = request
            .queries
            .queries()
            .iter()
            .map(|q| (q.original().name().clone(), q.query_type()))
            .collect();
        let mut reply = {
            let settings = self.settings.borrow().clone();
            let view = self.live.read();
            answer(request.metadata.op_code, &questions, &view, &settings)
        };
        let udp = udp_limit(request.edns.as_ref().map(|e| e.max_payload()));
        // hickory-server uses the response's EDNS size as the UDP limit.
        let edns = request.edns.as_ref().map(|_| {
            let mut e = Edns::new();
            e.set_max_payload(udp);
            e
        });
        let limit = match request.protocol() {
            Protocol::Udp => udp,
            _ => u16::MAX,
        };
        let truncated = questions.len() == 1
            && fit_response(&mut reply, &request.queries, edns.as_ref(), limit);
        let mut metadata = Metadata::response_from_request(&request.metadata);
        metadata.truncation = truncated;
        metadata.response_code = reply.rcode;
        metadata.authoritative = reply.authoritative;
        let mut builder = MessageResponseBuilder::from_message_request(request);
        if let Some(e) = &edns {
            builder.edns(e);
        }
        let response = builder.build(
            metadata,
            reply.answers.iter(),
            std::iter::empty(),
            reply.soa.iter(),
            reply.additionals.iter(),
        );
        match response_handle.send_response(response).await {
            Ok(info) => info,
            Err(e) => {
                tracing::debug!("cannot send DNS response: {e}");
                metadata.response_code = ResponseCode::ServFail;
                hickory_proto::op::Header {
                    metadata,
                    counts: Default::default(),
                }
                .into()
            }
        }
    }
}

/// Bind every listen address (UDP and TCP) and start serving. Errors name the address.
pub async fn serve(
    listen: &[SocketAddr],
    handler: Handler,
) -> Result<hickory_server::Server<Handler>, String> {
    let mut server = hickory_server::Server::new(handler);
    for addr in listen {
        let udp = tokio::net::UdpSocket::bind(addr)
            .await
            .map_err(|e| format!("cannot listen on {addr} (udp): {e}"))?;
        let tcp = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("cannot listen on {addr} (tcp): {e}"))?;
        server.register_socket(udp);
        server.register_listener(tcp, Duration::from_secs(5), 64);
    }
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::LiveService;
    use HealthState::*;
    use emfor_format::{BackendRecord, Balance, HealthState, OnAllDown, ServiceRecord};
    use hickory_proto::op::{Message, Query};

    fn settings() -> DnsSettings {
        DnsSettings {
            zone: "svc.emfor".into(),
            ttl: 1,
            serial: 1762905600,
        }
    }

    fn service(
        name: &str,
        backends: &[(&str, HealthState)],
        on_all_down: OnAllDown,
        balance: Balance,
        dns_ttl: Option<u32>,
    ) -> (String, LiveService) {
        let mut record = ServiceRecord {
            name: name.into(),
            on_all_down,
            balance,
            all_down: true,
            pool: None,
            backends: backends
                .iter()
                .map(|(ip, state)| BackendRecord {
                    id: format!("{ip}:80"),
                    ip: ip.to_string(),
                    port: 80,
                    weight: 1,
                    site: None,
                    state: *state,
                    checked_at: None,
                    reason: None,
                    latency_us: None,
                })
                .collect(),
        };
        record.refresh_all_down();
        (name.to_string(), LiveService::new(record, dns_ttl))
    }

    fn view(services: Vec<(String, LiveService)>) -> LiveView {
        LiveView {
            ready: true,
            services: services.into_iter().collect(),
            site: None,
            ..LiveView::default()
        }
    }

    /// The A answers of `n` queries for `name`.
    fn a_answers(v: &LiveView, name: &str, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| ask(v, name, RecordType::A).answers[0].data.to_string())
            .collect()
    }

    fn ask(v: &LiveView, name: &str, t: RecordType) -> Reply {
        let q = [(Name::from_ascii(name).unwrap(), t)];
        answer(OpCode::Query, &q, v, &settings())
    }

    /// rcode, answer rdata as text, has SOA in authority.
    type Summary = (ResponseCode, Vec<String>, bool);

    /// Short form of a reply.
    fn summary(r: &Reply) -> Summary {
        (
            r.rcode,
            r.answers.iter().map(|a| a.data.to_string()).collect(),
            r.soa.is_some(),
        )
    }

    #[test]
    fn healthy_backend_answered() {
        let v = view(vec![service(
            "api.svc.emfor",
            &[("10.0.0.11", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let r = ask(&v, "api.svc.emfor", RecordType::A);
        assert_eq!(
            summary(&r),
            (ResponseCode::NoError, vec!["10.0.0.11".into()], false)
        );
        assert!(r.authoritative);
        assert_eq!(r.answers[0].ttl, 1);
        assert_eq!(r.answers[0].record_type(), RecordType::A);
    }

    #[test]
    fn replies_by_case_example() {
        use OnAllDown::*;
        let nx = |c| (c, Vec::<String>::new(), true);
        let rows: Vec<(&str, HealthState, OnAllDown, RecordType, Summary)> = vec![
            (
                "10.0.0.11",
                Healthy,
                Open,
                RecordType::A,
                (ResponseCode::NoError, vec!["10.0.0.11".into()], false),
            ),
            (
                "10.0.0.11",
                Healthy,
                Open,
                RecordType::AAAA,
                nx(ResponseCode::NoError),
            ),
            (
                "2001:db8::11",
                Healthy,
                Open,
                RecordType::AAAA,
                (ResponseCode::NoError, vec!["2001:db8::11".into()], false),
            ),
            (
                "10.0.0.11",
                Unhealthy,
                Open,
                RecordType::A,
                (ResponseCode::NoError, vec!["10.0.0.11".into()], false),
            ),
            (
                "10.0.0.11",
                Unhealthy,
                Closed,
                RecordType::A,
                (ResponseCode::ServFail, vec![], false),
            ),
            (
                "10.0.0.11",
                Healthy,
                Open,
                RecordType::MX,
                nx(ResponseCode::NoError),
            ),
        ];
        for (ip, state, oad, t, want) in rows {
            let v = view(vec![service(
                "api.svc.emfor",
                &[(ip, state)],
                oad,
                Balance::RoundRobin,
                None,
            )]);
            assert_eq!(
                summary(&ask(&v, "api.svc.emfor", t)),
                want,
                "{ip} {state:?} {oad:?} {t}"
            );
        }
    }

    #[test]
    fn per_service_ttl() {
        let v = view(vec![service(
            "api.svc.emfor",
            &[("10.0.0.11", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            Some(5),
        )]);
        assert_eq!(ask(&v, "api.svc.emfor", RecordType::A).answers[0].ttl, 5);
    }

    #[test]
    fn round_robin_is_global_and_failover_is_fixed() {
        let rr = view(vec![service(
            "api.svc.emfor",
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let q = [(Name::from_ascii("api.svc.emfor").unwrap(), RecordType::A)];
        let picks: Vec<String> = (0..4)
            .map(|_| {
                answer(OpCode::Query, &q, &rr, &settings()).answers[0]
                    .data
                    .to_string()
            })
            .collect();
        assert_eq!(picks, ["10.0.0.1", "10.0.0.2", "10.0.0.1", "10.0.0.2"]);
        let fo = view(vec![service(
            "api.svc.emfor",
            &[
                ("10.0.0.1", Unhealthy),
                ("10.0.0.2", Healthy),
                ("10.0.0.3", Healthy),
            ],
            OnAllDown::Open,
            Balance::Failover,
            None,
        )]);
        for _ in 0..3 {
            assert_eq!(
                answer(OpCode::Query, &q, &fo, &settings()).answers[0]
                    .data
                    .to_string(),
                "10.0.0.2"
            );
        }
    }

    #[test]
    fn weighted_proportions_over_one_cycle() {
        let (name, mut svc) = service(
            "api.svc.emfor",
            &[
                ("10.0.0.1", Healthy),
                ("10.0.0.2", Healthy),
                ("10.0.0.3", Healthy),
            ],
            OnAllDown::Open,
            Balance::Weighted,
            None,
        );
        svc.record.backends[1].weight = 3;
        let v = view(vec![(name, svc)]);
        let got = a_answers(&v, "api.svc.emfor", 5);
        let count = |ip: &str| got.iter().filter(|a| *a == ip).count();
        assert_eq!(
            (count("10.0.0.1"), count("10.0.0.2"), count("10.0.0.3")),
            (1, 3, 1),
            "{got:?}"
        );
    }

    /// Spec "Weight change applies on reload": after the record changes from
    /// weights 1:1 to 1:3 (the choice state is kept across reloads), the next
    /// four answers follow the new weights.
    #[test]
    fn weight_change_applies_to_answers() {
        let (name, old) = service(
            "api.svc.emfor",
            &[("10.0.0.1", Healthy), ("10.0.0.2", Healthy)],
            OnAllDown::Open,
            Balance::Weighted,
            None,
        );
        let v = view(vec![(name.clone(), old.clone())]);
        a_answers(&v, "api.svc.emfor", 3);
        let mut new = old.clone();
        new.record.backends[1].weight = 4;
        assert!(std::sync::Arc::ptr_eq(&new.round_robin, &old.round_robin));
        let v = view(vec![(name, new)]);
        let got = a_answers(&v, "api.svc.emfor", 5);
        let count = |ip: &str| got.iter().filter(|a| *a == ip).count();
        assert_eq!((count("10.0.0.1"), count("10.0.0.2")), (1, 4), "{got:?}");
    }

    #[test]
    fn locality_answers_only_local_backends() {
        let (name, mut svc) = service(
            "api.svc.emfor",
            &[
                ("10.0.0.1", Healthy),
                ("10.0.0.2", Healthy),
                ("10.0.0.3", Healthy),
            ],
            OnAllDown::Open,
            Balance::Locality,
            None,
        );
        for (b, site) in svc.record.backends.iter_mut().zip(["dc1", "dc1", "dc2"]) {
            b.site = Some(site.into());
        }
        let mut v = view(vec![(name, svc)]);
        v.site = Some("dc1".into());
        assert_eq!(
            a_answers(&v, "api.svc.emfor", 4),
            ["10.0.0.1", "10.0.0.2", "10.0.0.1", "10.0.0.2"]
        );
    }

    #[test]
    fn name_classification_example() {
        let v = view(vec![service(
            "a.b.svc.emfor",
            &[("10.0.0.1", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let none = Vec::<String>::new();
        let rows: Vec<(&str, RecordType, Summary)> = vec![
            (
                "example.com",
                RecordType::A,
                (ResponseCode::Refused, none.clone(), false),
            ),
            (
                "svc.emfor",
                RecordType::A,
                (ResponseCode::NoError, none.clone(), true),
            ),
            (
                "b.svc.emfor",
                RecordType::A,
                (ResponseCode::NoError, none.clone(), true),
            ),
            (
                "c.svc.emfor",
                RecordType::A,
                (ResponseCode::NXDomain, none.clone(), true),
            ),
            (
                "nope.svc.emfor",
                RecordType::A,
                (ResponseCode::NXDomain, none.clone(), true),
            ),
            (
                "A.B.SVC.EMFOR.",
                RecordType::A,
                (ResponseCode::NoError, vec!["10.0.0.1".into()], false),
            ),
        ];
        for (name, t, want) in rows {
            let r = ask(&v, name, t);
            assert_eq!(summary(&r), want, "{name} {t}");
            if r.rcode != ResponseCode::Refused {
                assert!(r.authoritative, "{name}");
            }
        }
        // The reply echoes the name as received.
        let r = ask(&v, "A.B.SVC.EMFOR.", RecordType::A);
        assert_eq!(r.answers[0].name.to_ascii(), "A.B.SVC.EMFOR.");
        // Apex SOA query.
        let r = ask(&v, "svc.emfor", RecordType::SOA);
        assert_eq!(r.rcode, ResponseCode::NoError);
        assert_eq!(r.answers.len(), 1);
        assert_eq!(r.answers[0].record_type(), RecordType::SOA);
    }

    #[test]
    fn soa_values() {
        let rec = soa_record(&settings());
        assert_eq!(rec.name.to_ascii(), "svc.emfor.");
        assert_eq!(rec.ttl, 1);
        let RData::SOA(soa) = rec.data else {
            panic!("not SOA")
        };
        assert_eq!(soa.mname.to_ascii(), "ns.svc.emfor.");
        assert_eq!(soa.rname.to_ascii(), "hostmaster.svc.emfor.");
        assert_eq!(soa.serial, 1762905600);
        assert_eq!(
            (soa.refresh, soa.retry, soa.expire, soa.minimum),
            (3600, 600, 86400, 1)
        );
    }

    #[test]
    fn malformed_and_unsupported() {
        let v = LiveView::default();
        let s = settings();
        let q = (Name::from_ascii("svc.emfor").unwrap(), RecordType::SOA);
        assert_eq!(
            answer(OpCode::Update, std::slice::from_ref(&q), &v, &s).rcode,
            ResponseCode::NotImp
        );
        assert_eq!(
            answer(OpCode::Query, &[], &v, &s).rcode,
            ResponseCode::FormErr
        );
        assert_eq!(
            answer(OpCode::Query, &[q.clone(), q], &v, &s).rcode,
            ResponseCode::FormErr
        );
    }

    #[test]
    fn servfail_until_first_layout() {
        let mut v = view(vec![service(
            "api.svc.emfor",
            &[("10.0.0.1", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        v.ready = false;
        assert_eq!(
            ask(&v, "api.svc.emfor", RecordType::A).rcode,
            ResponseCode::ServFail
        );
        assert_eq!(
            ask(&v, "nope.svc.emfor", RecordType::A).rcode,
            ResponseCode::ServFail
        );
        assert_eq!(
            ask(&v, "example.com", RecordType::A).rcode,
            ResponseCode::Refused
        );
    }

    #[test]
    fn service_outside_zone_is_refused() {
        let v = view(vec![service(
            "db.internal",
            &[("10.0.0.1", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        assert_eq!(
            ask(&v, "db.internal", RecordType::A).rcode,
            ResponseCode::Refused
        );
    }

    /// A service whose backends are `ip:port` strings (IPv6 bracketed).
    fn srv_service(
        name: &str,
        backends: &[(&str, HealthState)],
        on_all_down: OnAllDown,
        balance: Balance,
        dns_ttl: Option<u32>,
    ) -> (String, LiveService) {
        let (n, mut s) = service(name, &[], on_all_down, balance, dns_ttl);
        s.record.backends = backends
            .iter()
            .map(|(addr, state)| {
                let (ip, port) = addr.rsplit_once(':').unwrap();
                BackendRecord {
                    id: addr.to_string(),
                    ip: ip.trim_matches(['[', ']']).to_string(),
                    port: port.parse().unwrap(),
                    weight: 1,
                    site: None,
                    state: *state,
                    checked_at: None,
                    reason: None,
                    latency_us: None,
                }
            })
            .collect();
        s.record.refresh_all_down();
        (n, s)
    }

    fn text(records: &[Record]) -> Vec<String> {
        records.iter().map(|a| a.data.to_string()).collect()
    }

    #[test]
    fn ip_labels() {
        let l = |s: &str| ip_label(s.parse().unwrap());
        assert_eq!(l("10.0.0.11"), "10-0-0-11");
        assert_eq!(l("2001:db8::11"), "2001-0db8-0000-0000-0000-0000-0000-0011");
        assert_eq!(l("::1"), "0000-0000-0000-0000-0000-0000-0000-0001");
        assert_eq!(l("2001:DB8::"), "2001-0db8-0000-0000-0000-0000-0000-0000");
    }

    #[test]
    fn target_labels_example() {
        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[
                ("10.0.0.11:8080", Healthy),
                ("[2001:db8::11]:8080", Healthy),
                ("[::1]:8080", Healthy),
            ],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let nodata = (ResponseCode::NoError, Vec::<String>::new(), true);
        let nx = (ResponseCode::NXDomain, Vec::<String>::new(), true);
        let ok = |a: &str| (ResponseCode::NoError, vec![a.to_string()], false);
        let rows: &[(&str, RecordType, Summary)] = &[
            ("10-0-0-11.api.svc.emfor", RecordType::A, ok("10.0.0.11")),
            ("10-0-0-11.api.svc.emfor", RecordType::AAAA, nodata.clone()),
            ("010-0-0-11.api.svc.emfor", RecordType::A, nx.clone()),
            (
                "2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor",
                RecordType::AAAA,
                ok("2001:db8::11"),
            ),
            ("2001-db8--11.api.svc.emfor", RecordType::AAAA, nx.clone()),
            (
                "0000-0000-0000-0000-0000-0000-0000-0001.api.svc.emfor",
                RecordType::AAAA,
                ok("::1"),
            ),
            ("10-0-0-11.api.svc.emfor", RecordType::SRV, nodata.clone()),
            ("10-0-0-99.api.svc.emfor", RecordType::A, nx.clone()),
            ("10-0-0-11.API.SVC.EMFOR.", RecordType::A, ok("10.0.0.11")),
        ];
        for (name, t, want) in rows {
            let r = ask(&v, name, *t);
            assert_eq!(&summary(&r), want, "{name} {t}");
            if r.rcode == ResponseCode::NoError || r.rcode == ResponseCode::NXDomain {
                assert!(r.authoritative, "{name} {t}");
            }
        }
    }

    #[test]
    fn target_of_down_backend() {
        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[("10.0.0.11:8080", Unhealthy)],
            OnAllDown::Closed,
            Balance::RoundRobin,
            Some(5),
        )]);
        assert_eq!(
            ask(&v, "api.svc.emfor", RecordType::A).rcode,
            ResponseCode::ServFail
        );
        let r = ask(&v, "10-0-0-11.api.svc.emfor", RecordType::A);
        assert_eq!(
            summary(&r),
            (ResponseCode::NoError, vec!["10.0.0.11".into()], false)
        );
        assert_eq!(r.answers[0].ttl, 5);
    }

    #[test]
    fn srv_on_service_name() {
        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[("10.0.0.11:8080", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let r = ask(&v, "api.svc.emfor", RecordType::SRV);
        assert_eq!(
            summary(&r),
            (
                ResponseCode::NoError,
                vec!["0 1 8080 10-0-0-11.api.svc.emfor.".into()],
                false
            )
        );
        assert!(r.authoritative);
        assert_eq!(r.answers[0].name.to_ascii(), "api.svc.emfor");
        assert_eq!(text(&r.additionals), ["10.0.0.11"]);
        assert_eq!(r.additionals[0].name.to_ascii(), "10-0-0-11.api.svc.emfor.");
    }

    #[test]
    fn srv_related_names_example() {
        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[("10.0.0.11:8080", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let srv = (
            ResponseCode::NoError,
            vec!["0 1 8080 10-0-0-11.api.svc.emfor.".to_string()],
            false,
        );
        let nodata = (ResponseCode::NoError, Vec::<String>::new(), true);
        let nx = (ResponseCode::NXDomain, Vec::<String>::new(), true);
        let rows: &[(&str, RecordType, Summary)] = &[
            ("_http._tcp.api.svc.emfor", RecordType::SRV, srv.clone()),
            ("_HTTP._TCP.API.SVC.EMFOR.", RecordType::SRV, srv.clone()),
            ("_http._tcp.api.svc.emfor", RecordType::TXT, nodata.clone()),
            ("_http._tcp.api.svc.emfor", RecordType::A, nodata.clone()),
            ("_tcp.api.svc.emfor", RecordType::SRV, nodata.clone()),
            ("_tcp.api.svc.emfor", RecordType::A, nodata.clone()),
            ("_http._udp.api.svc.emfor", RecordType::SRV, nx.clone()),
            ("_._tcp.api.svc.emfor", RecordType::SRV, nx.clone()),
            ("x._http._tcp.api.svc.emfor", RecordType::SRV, nx.clone()),
            ("_http._tcp.nope.svc.emfor", RecordType::SRV, nx.clone()),
            (
                "10-0-0-11.api.svc.emfor",
                RecordType::A,
                (ResponseCode::NoError, vec!["10.0.0.11".into()], false),
            ),
            ("10-0-0-99.api.svc.emfor", RecordType::A, nx.clone()),
        ];
        for (name, t, want) in rows {
            assert_eq!(&summary(&ask(&v, name, *t)), want, "{name} {t}");
        }
        // Owner name of the answer is the query name as received.
        let r = ask(&v, "_HTTP._TCP.API.SVC.EMFOR.", RecordType::SRV);
        assert_eq!(r.answers[0].name.to_ascii(), "_HTTP._TCP.API.SVC.EMFOR.");
    }

    #[test]
    fn service_name_wins_over_target_form() {
        let v = view(vec![
            srv_service(
                "api.svc.emfor",
                &[("10.0.0.11:8080", Healthy)],
                OnAllDown::Open,
                Balance::RoundRobin,
                None,
            ),
            srv_service(
                "10-0-0-11.api.svc.emfor",
                &[("10.9.9.9:80", Healthy)],
                OnAllDown::Open,
                Balance::RoundRobin,
                None,
            ),
        ]);
        assert_eq!(
            summary(&ask(&v, "10-0-0-11.api.svc.emfor", RecordType::A)).1,
            ["10.9.9.9"]
        );
    }

    #[test]
    fn srv_both_families_shared_ip_and_ttl() {
        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[
                ("10.0.0.11:8080", Healthy),
                ("[2001:db8::11]:8080", Healthy),
            ],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let r = ask(&v, "api.svc.emfor", RecordType::SRV);
        assert_eq!(
            text(&r.answers),
            [
                "0 1 8080 10-0-0-11.api.svc.emfor.",
                "0 1 8080 2001-0db8-0000-0000-0000-0000-0000-0011.api.svc.emfor."
            ]
        );
        assert_eq!(text(&r.additionals), ["10.0.0.11", "2001:db8::11"]);
        assert_eq!(r.additionals[1].record_type(), RecordType::AAAA);

        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[("10.0.0.11:8080", Healthy), ("10.0.0.11:8081", Healthy)],
            OnAllDown::Open,
            Balance::Failover,
            Some(5),
        )]);
        let r = ask(&v, "api.svc.emfor", RecordType::SRV);
        assert_eq!(
            text(&r.answers),
            [
                "0 1 8080 10-0-0-11.api.svc.emfor.",
                "1 1 8081 10-0-0-11.api.svc.emfor."
            ]
        );
        assert_eq!(text(&r.additionals), ["10.0.0.11"]);
        assert!(r.answers.iter().chain(&r.additionals).all(|x| x.ttl == 5));
    }

    #[test]
    fn srv_all_down_closed_is_servfail() {
        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[("10.0.0.11:8080", Unhealthy)],
            OnAllDown::Closed,
            Balance::RoundRobin,
            None,
        )]);
        assert_eq!(
            ask(&v, "_http._tcp.api.svc.emfor", RecordType::SRV).rcode,
            ResponseCode::ServFail
        );
    }

    #[test]
    fn size_limit_example() {
        assert_eq!(udp_limit(None), 512);
        assert_eq!(udp_limit(Some(400)), 512);
        assert_eq!(udp_limit(Some(1232)), 1232);
        assert_eq!(udp_limit(Some(4096)), 1232);
    }

    /// Service `api.svc.emfor` with `n` healthy IPv4 backends on port 8080.
    fn many(n: usize) -> LiveView {
        let addrs: Vec<String> = (1..=n)
            .map(|i| format!("10.0.{}.{}:8080", i / 250, i % 250 + 1))
            .collect();
        let backends: Vec<(&str, HealthState)> =
            addrs.iter().map(|a| (a.as_str(), Healthy)).collect();
        view(vec![srv_service(
            "api.svc.emfor",
            &backends,
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )])
    }

    /// The question section of an SRV query for `api.svc.emfor`.
    fn srv_queries() -> Queries {
        use hickory_proto::serialize::binary::{BinDecoder, BinEncodable};
        let q = Query::query(Name::from_ascii("api.svc.emfor.").unwrap(), RecordType::SRV);
        let bytes = q.to_bytes().unwrap();
        Queries::read(&mut BinDecoder::new(&bytes), 1).unwrap()
    }

    /// Largest backend count whose SRV response fits `limit` with glue,
    /// and without glue.
    fn srv_fit_counts(edns: Option<&Edns>, limit: usize) -> (usize, usize) {
        let q = srv_queries();
        let (mut with, mut without) = (0, 0);
        for n in 1..=80 {
            let mut r = ask(&many(n), "api.svc.emfor", RecordType::SRV);
            if encoded_len(&r, &q, edns) <= limit {
                with = n;
            }
            r.additionals.clear();
            if encoded_len(&r, &q, edns) <= limit {
                without = n;
            }
        }
        (with, without)
    }

    /// Measured boundaries for IPv4 targets under `api.svc.emfor` (they
    /// replace the estimates in the emfor-dns-srv design).
    #[test]
    fn srv_size_boundaries() {
        let mut e = Edns::new();
        e.set_max_payload(1232);
        let no_edns = srv_fit_counts(None, 512);
        let edns = srv_fit_counts(Some(&e), 1232);
        assert_eq!((no_edns, edns), ((8, 11), (19, 27)), "measured");
    }

    #[test]
    fn glue_dropped_when_it_does_not_fit() {
        let q = srv_queries();
        let (with, without) = srv_fit_counts(None, 512);
        assert!(with < without);
        let mut fits = ask(&many(with), "api.svc.emfor", RecordType::SRV);
        assert!(!fit_response(&mut fits, &q, None, 512));
        assert_eq!(fits.additionals.len(), with);
        let mut r = ask(&many(with + 1), "api.svc.emfor", RecordType::SRV);
        assert!(!fit_response(&mut r, &q, None, 512));
        assert!(r.additionals.is_empty());
        assert_eq!(r.answers.len(), with + 1);
        // SRV records alone overflow: as many as fit, truncated.
        let mut r = ask(&many(without + 1), "api.svc.emfor", RecordType::SRV);
        assert!(fit_response(&mut r, &q, None, 512));
        assert!(r.additionals.is_empty());
        assert_eq!(r.answers.len(), without);
        // TCP: no trimming.
        let mut r = ask(&many(without + 1), "api.svc.emfor", RecordType::SRV);
        assert!(!fit_response(&mut r, &q, None, u16::MAX));
        assert_eq!(r.additionals.len(), without + 1);
    }

    /// Serve `v` on ephemeral UDP and TCP ports of 127.0.0.1.
    async fn live_server(v: LiveView) -> (hickory_server::Server<Handler>, SocketAddr, SocketAddr) {
        let live = LiveState::default();
        live.replace(v.services, v.site);
        let (_tx, rx) = watch::channel(settings());
        std::mem::forget(_tx);
        let mut server = hickory_server::Server::new(Handler::new(live, rx));
        let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (ua, ta) = (udp.local_addr().unwrap(), tcp.local_addr().unwrap());
        server.register_socket(udp);
        server.register_listener(tcp, Duration::from_secs(5), 64);
        (server, ua, ta)
    }

    fn srv_query(edns: Option<u16>) -> Vec<u8> {
        let mut m = Message::query();
        m.metadata.id = 7;
        m.add_query(Query::query(
            Name::from_ascii("api.svc.emfor.").unwrap(),
            RecordType::SRV,
        ));
        if let Some(size) = edns {
            let mut e = Edns::new();
            e.set_max_payload(size);
            m.set_edns(e);
        }
        m.to_vec().unwrap()
    }

    async fn over_udp(addr: SocketAddr, query: &[u8]) -> (usize, Message) {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sock.send_to(query, addr).await.unwrap();
        let mut buf = vec![0u8; 65535];
        let n = tokio::time::timeout(Duration::from_secs(5), sock.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        (n, Message::from_vec(&buf[..n]).unwrap())
    }

    async fn over_tcp(addr: SocketAddr, query: &[u8]) -> Message {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut framed = (query.len() as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(query);
        s.write_all(&framed).await.unwrap();
        let mut len = [0u8; 2];
        s.read_exact(&mut len).await.unwrap();
        let mut buf = vec![0u8; usize::from(u16::from_be_bytes(len))];
        s.read_exact(&mut buf).await.unwrap();
        Message::from_vec(&buf).unwrap()
    }

    #[tokio::test]
    async fn srv_over_the_wire_fits_the_transport() {
        let (with, _) = srv_fit_counts(None, 512);
        // Glue dropped without truncation.
        let (_s, udp, _) = live_server(many(with + 1)).await;
        let (n, m) = over_udp(udp, &srv_query(None)).await;
        assert!(n <= 512, "{n} bytes");
        assert!(!m.metadata.truncation);
        assert_eq!(m.answers.len(), with + 1);
        assert!(m.additionals.is_empty());

        // SRV records alone overflow 512: TC over UDP, complete over TCP.
        let (_s, udp, tcp) = live_server(many(40)).await;
        let (n, m) = over_udp(udp, &srv_query(None)).await;
        assert!(n <= 512, "{n} bytes");
        assert!(m.metadata.truncation);
        assert_eq!(m.answers.len(), 11);
        assert!(m.additionals.is_empty());
        let m = over_tcp(tcp, &srv_query(None)).await;
        assert!(!m.metadata.truncation);
        assert_eq!(m.answers.len(), 40);
        assert_eq!(m.additionals.len(), 40);

        // The client's EDNS size, clamped, is the limit and is echoed.
        let (n, m) = over_udp(udp, &srv_query(Some(4096))).await;
        assert!(n <= 1232, "{n} bytes");
        assert!(m.metadata.truncation);
        assert_eq!(m.answers.len(), 27);
        assert_eq!(m.max_payload(), 1232);
        let (n, m) = over_udp(udp, &srv_query(Some(600))).await;
        assert!(n <= 600, "{n} bytes");
        assert!(m.metadata.truncation);
        assert_eq!(m.edns.as_ref().map(Edns::max_payload), Some(600));
    }

    #[test]
    fn srv_target_too_long_is_servfail() {
        // 222 characters: a valid service name, but with a 39-character
        // IPv6 label the target name exceeds 255 bytes.
        let name = format!(
            "{}.{}.{}.{}.svc.emfor",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(20)
        );
        assert_eq!(name.len(), 222);
        let v = view(vec![srv_service(
            &name,
            &[
                ("10.0.0.11:8080", Healthy),
                ("[2001:db8::11]:8080", Healthy),
            ],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        assert_eq!(
            ask(&v, &name, RecordType::SRV).rcode,
            ResponseCode::ServFail
        );
        // A and AAAA on the service name are unaffected.
        assert_eq!(ask(&v, &name, RecordType::A).rcode, ResponseCode::NoError);
    }
}
