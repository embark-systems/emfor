//! DNS server: answers A and AAAA for services under the zone from live
//! health state. All behavior is in the pure function [`answer`]; the
//! hickory-server adapter only decodes and encodes.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hickory_proto::op::{OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, SOA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use hickory_server::net::runtime::Time;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;
use tokio::sync::watch;

use crate::config::in_zone;
use crate::live::{LiveState, LiveView};
use crate::select::{Family, Selection, select};

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

/// Round-robin positions per service and family, global across DNS clients.
#[derive(Debug, Default)]
pub struct Counters(Mutex<HashMap<(String, Family), Arc<AtomicU64>>>);

impl Counters {
    fn get(&self, service: &str, family: Family) -> Arc<AtomicU64> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.entry((service.to_string(), family)).or_default().clone()
    }
}

/// What to send back. Records carry the query name as received.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub rcode: ResponseCode,
    pub authoritative: bool,
    pub answers: Vec<Record>,
    /// Zone SOA for the authority section (NODATA and NXDOMAIN).
    pub soa: Option<Record>,
}

impl Reply {
    fn error(rcode: ResponseCode) -> Reply {
        Reply {
            rcode,
            authoritative: false,
            answers: Vec::new(),
            soa: None,
        }
    }

    fn authoritative(rcode: ResponseCode, answers: Vec<Record>, soa: Option<Record>) -> Reply {
        Reply {
            rcode,
            authoritative: true,
            answers,
            soa,
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

/// Compute the reply to one request.
pub fn answer(
    op_code: OpCode,
    questions: &[(Name, RecordType)],
    view: &LiveView,
    settings: &DnsSettings,
    counters: &Counters,
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
    let soa = soa_record(settings);
    let nodata = |soa: Record| Reply::authoritative(ResponseCode::NoError, Vec::new(), Some(soa));

    if name == zone && qtype == RecordType::SOA {
        return Reply::authoritative(ResponseCode::NoError, vec![soa], None);
    }
    if let Some(svc) = view.services.get(&name) {
        let family = match qtype {
            RecordType::A => Family::V4,
            RecordType::AAAA => Family::V6,
            _ => return nodata(soa),
        };
        let counter = counters.get(&name, family);
        return match select(&svc.record, family, &counter) {
            Selection::Chosen(ip) => {
                let ttl = svc.dns_ttl.unwrap_or(settings.ttl);
                let rdata = match ip {
                    IpAddr::V4(v4) => RData::A(A(v4)),
                    IpAddr::V6(v6) => RData::AAAA(AAAA(v6)),
                };
                let rec = Record::from_rdata(qname.clone(), ttl, rdata);
                Reply::authoritative(ResponseCode::NoError, vec![rec], None)
            }
            Selection::NoFamily => nodata(soa),
            Selection::AllDown => Reply::error(ResponseCode::ServFail),
        };
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

/// hickory-server adapter around [`answer`].
pub struct Handler {
    live: LiveState,
    settings: watch::Receiver<DnsSettings>,
    counters: Counters,
}

impl Handler {
    pub fn new(live: LiveState, settings: watch::Receiver<DnsSettings>) -> Handler {
        Handler {
            live,
            settings,
            counters: Counters::default(),
        }
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
        let reply = {
            let settings = self.settings.borrow().clone();
            let view = self.live.read();
            answer(
                request.metadata.op_code,
                &questions,
                &view,
                &settings,
                &self.counters,
            )
        };
        let mut metadata = hickory_proto::op::Metadata::response_from_request(&request.metadata);
        metadata.response_code = reply.rcode;
        metadata.authoritative = reply.authoritative;
        let mut builder = MessageResponseBuilder::from_message_request(request);
        let edns = request.edns.as_ref().map(|_| {
            let mut e = hickory_proto::op::Edns::new();
            e.set_max_payload(1232);
            e
        });
        if let Some(e) = &edns {
            builder.edns(e);
        }
        let response = builder.build(
            metadata,
            reply.answers.iter(),
            std::iter::empty(),
            reply.soa.iter(),
            std::iter::empty(),
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
            backends: backends
                .iter()
                .map(|(ip, state)| BackendRecord {
                    id: format!("{ip}:80"),
                    ip: ip.to_string(),
                    port: 80,
                    state: *state,
                    checked_at: None,
                    reason: None,
                })
                .collect(),
        };
        record.refresh_all_down();
        (name.to_string(), LiveService { record, dns_ttl })
    }

    fn view(services: Vec<(String, LiveService)>) -> LiveView {
        LiveView {
            services: services.into_iter().collect(),
        }
    }

    fn ask(v: &LiveView, name: &str, t: RecordType) -> Reply {
        let q = [(Name::from_ascii(name).unwrap(), t)];
        answer(OpCode::Query, &q, v, &settings(), &Counters::default())
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
        let counters = Counters::default();
        let q = [(Name::from_ascii("api.svc.emfor").unwrap(), RecordType::A)];
        let picks: Vec<String> = (0..4)
            .map(|_| {
                answer(OpCode::Query, &q, &rr, &settings(), &counters).answers[0]
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
                answer(OpCode::Query, &q, &fo, &settings(), &counters).answers[0]
                    .data
                    .to_string(),
                "10.0.0.2"
            );
        }
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
        let c = Counters::default();
        let q = (Name::from_ascii("svc.emfor").unwrap(), RecordType::SOA);
        assert_eq!(
            answer(OpCode::Update, std::slice::from_ref(&q), &v, &s, &c).rcode,
            ResponseCode::NotImp
        );
        assert_eq!(
            answer(OpCode::Query, &[], &v, &s, &c).rcode,
            ResponseCode::FormErr
        );
        assert_eq!(
            answer(OpCode::Query, &[q.clone(), q], &v, &s, &c).rcode,
            ResponseCode::FormErr
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
}
