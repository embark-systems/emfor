//! Fuzzing entry point of the DNS server (`fuzz/fuzz_targets/dns.rs`),
//! built only with `--cfg fuzzing`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::OnceLock;

use emfor_format::{BackendRecord, Balance, HealthState, OnAllDown, ServiceRecord};
use hickory_proto::op::MessageType;
use hickory_proto::rr::Record;
use hickory_proto::serialize::binary::BinEncoder;
use hickory_server::net::NetError;
use hickory_server::net::runtime::TokioTime;
use hickory_server::net::xfer::Protocol;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponse;
use tokio::sync::watch;

use super::{DnsSettings, Handler, udp_limit};
use crate::config::MAX_ZONE_LEN;
use crate::live::{LiveService, LiveState};

/// Answer `data` the way the DNS server answers a request. The first byte
/// picks how: bit 0 set, over TCP, else over UDP; bit 1 set, in the longest
/// zone allowed, else in `svc.emfor`; bit 2 set, before the first layout,
/// else with the services of [`services`]. The rest is the request as it
/// arrives (for TCP, without the length prefix). `fuzz/seeds/dns.py` writes
/// seeds for it.
///
/// Panics when a response to a request with one question does not fit the
/// transport's size limit: [`super::fit_response`] promises that it does.
pub fn fuzz_request(data: &[u8]) {
    let Some((&mode, raw)) = data.split_first() else {
        return;
    };
    let protocol = if mode & 1 == 0 {
        Protocol::Udp
    } else {
        Protocol::Tcp
    };
    let src = SocketAddr::from(([127, 0, 0, 1], 5353));
    // As hickory-server: a request that does not decode gets FORMERR from
    // hickory itself, and a response is never answered.
    let Ok(request) = Request::from_bytes(raw.to_vec(), src, protocol) else {
        return;
    };
    if request.metadata.message_type == MessageType::Response {
        return;
    }
    let long_zone = mode & 2 != 0;
    let ready = mode & 4 == 0;
    let handler = handler(long_zone, ready);
    let sink = Sink {
        limit: match protocol {
            Protocol::Udp => udp_limit(request.edns.as_ref().map(|e| e.max_payload())),
            _ => u16::MAX,
        },
        must_fit: request.queries.queries().len() == 1,
    };
    futures::executor::block_on(handler.handle_request::<_, TokioTime>(&request, sink));
}

/// Encodes the response as hickory-server does before sending it.
#[derive(Clone)]
struct Sink {
    limit: u16,
    must_fit: bool,
}

#[async_trait::async_trait]
impl ResponseHandler for Sink {
    async fn send_response<'a>(
        &mut self,
        response: MessageResponse<
            '_,
            'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
            impl Iterator<Item = &'a Record> + Send + 'a,
        >,
    ) -> Result<ResponseInfo, NetError> {
        let mut bytes = Vec::with_capacity(512);
        let mut encoder = BinEncoder::new(&mut bytes);
        encoder.set_max_size(self.limit);
        match response.destructive_emit(&mut encoder) {
            Ok(info) => Ok(info),
            Err(e) => {
                assert!(
                    !self.must_fit,
                    "a response to one question does not fit in {} bytes: {e}",
                    self.limit
                );
                Err(e.into())
            }
        }
    }
}

/// One handler per zone and readiness, built once.
fn handler(long_zone: bool, ready: bool) -> &'static Handler {
    static HANDLERS: OnceLock<[Handler; 4]> = OnceLock::new();
    let all = HANDLERS.get_or_init(|| {
        [(false, true), (true, true), (false, false), (true, false)].map(|(long, ready)| {
            build(
                if long {
                    long_name()
                } else {
                    "svc.emfor".into()
                },
                ready,
            )
        })
    });
    &all[usize::from(long_zone) + 2 * usize::from(!ready)]
}

/// A zone of [`MAX_ZONE_LEN`] characters, in labels of at most 60.
fn long_name() -> String {
    let mut z = String::new();
    while z.len() < MAX_ZONE_LEN {
        if !z.is_empty() {
            z.push('.');
        }
        let n = (MAX_ZONE_LEN - z.len()).min(60);
        z.push_str(&"a".repeat(n));
    }
    z
}

fn build(zone: String, ready: bool) -> Handler {
    let live = LiveState::default();
    if ready {
        let services = services(&zone)
            .into_iter()
            .map(|(record, ttl)| (record.name.clone(), LiveService::new(record, ttl)))
            .collect::<HashMap<_, _>>();
        live.replace(services, Some("dc1".into()));
    }
    let (_tx, rx) = watch::channel(DnsSettings {
        zone,
        ttl: 30,
        serial: 1762905600,
        rate_limit: 0,
    });
    // The receiver keeps the last value after the sender is gone.
    Handler::new(live, rx)
}

fn backend(addr: &str, state: HealthState, weight: u32, site: Option<&str>) -> BackendRecord {
    let (ip, port) = addr
        .rsplit_once(':')
        .expect("the fuzz target's backends are written as ip:port");
    BackendRecord {
        id: addr.into(),
        ip: ip.trim_matches(['[', ']']).into(),
        port: port
            .parse()
            .expect("the fuzz target's backends have a valid port"),
        weight,
        site: site.map(Into::into),
        backup: false,
        state,
        checked_at: Some(1762905600123),
        reason: (state != HealthState::Healthy).then(|| "connect: Connection refused".into()),
        latency_us: None,
        recovered_at: None,
    }
}

/// Services covering each kind of answer: A and AAAA with several
/// methods, all down with `closed`, many SRV targets (truncation), and a
/// name long enough that one record overflows 512 bytes and an IPv6 SRV
/// target cannot be named.
fn services(zone: &str) -> Vec<(ServiceRecord, Option<u32>)> {
    use HealthState::{Healthy, Unhealthy, Unknown};
    let service = |name: String, on_all_down, balance, backends: Vec<BackendRecord>| {
        let mut r = ServiceRecord {
            name,
            on_all_down,
            balance,
            all_down: true,
            slow_start_ms: 0,
            backends,
            pool: None,
        };
        r.refresh_all_down();
        r
    };
    let many = (1..=40)
        .map(|i| backend(&format!("10.0.1.{i}:5060"), Healthy, i % 5 + 1, None))
        .collect();
    // 252 characters in `svc.emfor`; at most 253 in the long zone.
    let long = format!(
        "{}.{}.{}.{}.{zone}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(50)
    );
    let long = long[long.len().saturating_sub(253)..]
        .trim_start_matches('.')
        .to_owned();
    vec![
        (
            service(
                format!("api.{zone}"),
                OnAllDown::Open,
                Balance::RoundRobin,
                vec![
                    backend("10.0.0.11:8080", Healthy, 1, None),
                    backend("10.0.0.12:8080", Unhealthy, 1, None),
                    backend("[2001:db8::11]:8080", Healthy, 1, None),
                ],
            ),
            None,
        ),
        (
            service(
                format!("db.{zone}"),
                OnAllDown::Closed,
                Balance::Failover,
                vec![
                    backend("10.0.2.5:5432", Unhealthy, 1, None),
                    backend("[2001:db8::5]:5432", Unknown, 1, None),
                ],
            ),
            Some(5),
        ),
        (
            service(
                format!("web.{zone}"),
                OnAllDown::Open,
                Balance::Locality,
                vec![
                    backend("10.0.3.1:80", Healthy, 1, Some("dc1")),
                    backend("10.0.3.2:80", Healthy, 1, Some("dc2")),
                    backend("[2001:db8::31]:80", Unknown, 1, Some("dc1")),
                ],
            ),
            None,
        ),
        (
            service(
                format!("many.{zone}"),
                OnAllDown::Open,
                Balance::Weighted,
                many,
            ),
            Some(1),
        ),
        (
            service(
                long,
                OnAllDown::Open,
                Balance::RoundRobin,
                vec![
                    backend("10.0.4.1:443", Healthy, 1, None),
                    backend(
                        "[2001:db8:ffff:ffff:ffff:ffff:ffff:ffff]:443",
                        Healthy,
                        1,
                        None,
                    ),
                ],
            ),
            None,
        ),
    ]
}
