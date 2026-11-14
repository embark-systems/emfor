//! DNS server: answers A and AAAA for services under the zone from live
//! health state. All behavior is in the pure function [`answer`]; the
//! hickory-server adapter only decodes and encodes.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use hickory_proto::op::{Edns, Header, MessageType, Metadata, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, SOA, SRV};
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinDecoder, BinEncoder};
use hickory_server::net::runtime::iocompat::AsyncIoTokioAsStd;
use hickory_server::net::runtime::{Time, TokioTime};
use hickory_server::net::tcp::TcpStream;
use hickory_server::net::xfer::Protocol;
use hickory_server::server::{
    Request, RequestHandler, ResponseHandle, ResponseHandler, ResponseInfo, TimeoutStream,
};
use hickory_server::zone_handler::{MessageResponseBuilder, Queries};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use crate::config::in_zone;
use crate::live::{LiveService, LiveState, LiveView};
use crate::select::{Family, Selection, select, srv_targets};

// Fuzzing entry point (`fuzz/fuzz_targets/dns.rs`): a request through
// `Handler`, `answer` and `fit_response`.
#[cfg(fuzzing)]
#[path = "dns_fuzz.rs"]
mod fuzz;
#[cfg(fuzzing)]
pub use fuzz::fuzz_request;

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
    let mut n = Name::from_ascii(name)
        .expect("zone and service names are validated by the config (labels of [a-z0-9-], at most 242 characters for the zone)");
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

/// The SRV target name of a backend at `ip` under service `svc_name`:
/// `<ip_label>.<svc_name>.`. `None` when it is longer than a DNS name may
/// be, as with a long service name and a 39-character IPv6 label. The
/// config validation uses it too, so a service it accepts can always be
/// answered.
pub fn srv_target(ip: IpAddr, svc_name: &str) -> Option<Name> {
    let mut target = Name::from_ascii(format!("{}.{svc_name}.", ip_label(ip))).ok()?;
    target.set_fqdn(true);
    Some(target)
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
        // The config refuses a backend whose target name is too long; a
        // target that still cannot be named fails the whole answer.
        let Some(target) = srv_target(t.ip, svc_name) else {
            tracing::debug!("SRV target name for {} under {svc_name} is too long", t.ip);
            return Reply::error(ResponseCode::ServFail);
        };
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

/// Compute the reply to one request. `edns_version` is the version of the
/// request's OPT record, if it has one.
pub fn answer(
    op_code: OpCode,
    edns_version: Option<u8>,
    questions: &[(Name, RecordType, DNSClass)],
    view: &LiveView,
    settings: &DnsSettings,
) -> Reply {
    // RFC 6891 6.1.3: only EDNS version 0 is implemented.
    if edns_version.is_some_and(|v| v > 0) {
        return Reply::error(ResponseCode::BADVERS);
    }
    if op_code != OpCode::Query {
        return Reply::error(ResponseCode::NotImp);
    }
    let [(qname, qtype, class)] = questions else {
        return Reply::error(ResponseCode::FormErr);
    };
    // The zone exists in class IN only; any other class is refused like a
    // name outside the zone.
    if *class != DNSClass::IN {
        return Reply::error(ResponseCode::Refused);
    }
    let (qtype, name) = (*qtype, normalize(qname));
    let zone = settings.zone.as_str();
    // Label by label: in text form a `.` byte inside a label looks like a
    // label separator.
    if !fqdn(zone).zone_of(qname) {
        return Reply::error(ResponseCode::Refused);
    }
    if !view.ready {
        // Starting up: which names exist is not known yet. SERVFAIL is not
        // cached like NXDOMAIN would be.
        return Reply::error(ResponseCode::ServFail);
    }
    let soa = soa_record(settings);
    let nodata = |soa: Record| Reply::authoritative(ResponseCode::NoError, Vec::new(), Some(soa));
    // Configured names have labels of [a-z0-9-] (and `_` for SRV owners): a
    // label holding a `.` byte names none of them, nor an ancestor of one.
    // The checks below work on the text form, where it would look like two
    // labels.
    if qname.iter().any(|label| label.contains(&b'.')) {
        return Reply::authoritative(ResponseCode::NXDomain, Vec::new(), Some(soa));
    }

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
/// (missing glue never sets TC), then drop answers from the end, and last
/// the SOA in the authority section, until it fits. Returns true when
/// answers or the SOA were dropped, so the caller sets TC.
/// Encodes the response the way hickory-server does (same encoder, same
/// sections). Trimming answers here, not in hickory, keeps room for the
/// OPT record, which hickory leaves out when it cuts answers itself.
/// Even a single record can be too big: hickory does not compress names
/// against the question, so a name near 255 bytes, in the question and
/// again as the owner of an answer (or a zone that long, as the owner of
/// the SOA), goes over 512 bytes.
pub fn fit_response(reply: &mut Reply, queries: &Queries, edns: Option<&Edns>, limit: u16) -> bool {
    let limit = usize::from(limit);
    if encoded_len(reply, queries, edns) <= limit {
        return false;
    }
    reply.additionals.clear();
    if encoded_len(reply, queries, edns) <= limit {
        return false;
    }
    // Largest prefix of the answers that fits, by binary search: `hi` does
    // not fit, `lo` fits unless it is zero.
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
    if lo == 0 && encoded_len(reply, queries, edns) > limit {
        // Header, question (at most 259 bytes) and OPT always fit 512.
        reply.soa = None;
    }
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
#[derive(Clone)]
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
        let questions: Vec<(Name, RecordType, DNSClass)> = request
            .queries
            .queries()
            .iter()
            .map(|q| {
                let q = q.original();
                (q.name().clone(), q.query_type(), q.query_class())
            })
            .collect();
        let mut reply = {
            let settings = self.settings.borrow().clone();
            let view = self.live.read();
            let edns_version = request.edns.as_ref().map(Edns::version);
            answer(
                request.metadata.op_code,
                edns_version,
                &questions,
                &view,
                &settings,
            )
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

/// DNS-over-TCP connections served at once, over every listen address;
/// more are closed right after accept, so that clients cannot use up the
/// daemon's file descriptors (checks need them too).
pub const MAX_TCP_CONNECTIONS: usize = 128;
/// A TCP connection is closed when no complete request arrives for this
/// long, also when the client does not read its responses.
const TCP_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// Responses queued per TCP connection while the client is slow to read.
const TCP_RESPONSE_QUEUE: usize = 64;

/// The running DNS server. UDP is served by hickory-server, one server per
/// socket so that the end of any one is seen; TCP by [`tcp_listener`],
/// which limits the number of connections. Dropping it stops every
/// listener.
pub struct DnsServer {
    udp: Vec<(SocketAddr, hickory_server::Server<Handler>)>,
    tcp: JoinSet<()>,
    tcp_addrs: HashMap<tokio::task::Id, SocketAddr>,
}

impl DnsServer {
    /// Wait until a listener stops and describe it. hickory ends a UDP
    /// listener on a socket error it cannot recover from (for example
    /// `ConnectionAborted`); a TCP listener ends only by a panic. Pending
    /// otherwise. Cancel-safe.
    pub async fn ended(&mut self) -> String {
        let DnsServer {
            udp,
            tcp,
            tcp_addrs,
        } = self;
        let udp_end = async {
            if udp.is_empty() {
                return std::future::pending::<String>().await;
            }
            let (result, i, _) = futures::future::select_all(
                udp.iter_mut().map(|(_, s)| Box::pin(s.block_until_done())),
            )
            .await;
            let addr = udp[i].0;
            match result {
                Ok(()) => format!("DNS listener on {addr} (udp) stopped"),
                Err(e) => format!("DNS listener on {addr} (udp) stopped: {e}"),
            }
        };
        let tcp_end = async {
            let (id, how) = match tcp.join_next_with_id().await {
                None => return std::future::pending::<String>().await,
                Some(Ok((id, ()))) => (id, "stopped"),
                Some(Err(e)) => (e.id(), if e.is_panic() { "panicked" } else { "stopped" }),
            };
            match tcp_addrs.get(&id) {
                Some(addr) => format!("DNS listener on {addr} (tcp) {how}"),
                None => format!("DNS listener (tcp) {how}"),
            }
        };
        tokio::select! {
            m = udp_end => m,
            m = tcp_end => m,
        }
    }

    /// Stop every listener; open TCP connections are closed.
    pub async fn shutdown(mut self) {
        self.tcp.shutdown().await;
        for (addr, mut server) in self.udp {
            if let Err(e) = server.shutdown_gracefully().await {
                tracing::debug!("DNS server on {addr} (udp) shutdown: {e}");
            }
        }
    }
}

/// Bind every listen address (UDP and TCP) and start serving. Errors name the address.
pub async fn serve(listen: &[SocketAddr], handler: Handler) -> Result<DnsServer, String> {
    serve_with(listen, handler, TCP_IDLE_TIMEOUT).await
}

async fn serve_with(
    listen: &[SocketAddr],
    handler: Handler,
    idle: Duration,
) -> Result<DnsServer, String> {
    let mut sockets = Vec::with_capacity(listen.len());
    for addr in listen {
        let udp = tokio::net::UdpSocket::bind(addr)
            .await
            .map_err(|e| format!("cannot listen on {addr} (udp): {e}"))?;
        let tcp = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("cannot listen on {addr} (tcp): {e}"))?;
        // The bound addresses differ from `addr` only for port 0 (tests).
        let local = |r: std::io::Result<SocketAddr>| r.unwrap_or(*addr);
        sockets.push((local(udp.local_addr()), udp, local(tcp.local_addr()), tcp));
    }
    let permits = Arc::new(Semaphore::new(MAX_TCP_CONNECTIONS));
    let mut server = DnsServer {
        udp: Vec::with_capacity(sockets.len()),
        tcp: JoinSet::new(),
        tcp_addrs: HashMap::new(),
    };
    for (udp_addr, udp, tcp_addr, tcp) in sockets {
        let mut s = hickory_server::Server::new(handler.clone());
        s.register_socket(udp);
        server.udp.push((udp_addr, s));
        let task = server
            .tcp
            .spawn(tcp_listener(tcp, handler.clone(), permits.clone(), idle));
        server.tcp_addrs.insert(task.id(), tcp_addr);
    }
    Ok(server)
}

/// Accept DNS-over-TCP connections while a permit is free; close the
/// others at once. Never ends: accept errors (out of file descriptors and
/// the like) are waited out. The connections end with the listener.
async fn tcp_listener(
    listener: TcpListener,
    handler: Handler,
    permits: Arc<Semaphore>,
    idle: Duration,
) {
    let mut connections = JoinSet::new();
    loop {
        let accepted = listener.accept().await;
        while connections.try_join_next().is_some() {}
        let (stream, src) = match accepted {
            Ok(a) => a,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionAborted => continue,
            Err(e) => {
                tracing::debug!("DNS TCP accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            tracing::debug!(
                "DNS TCP connection from {src} closed: {MAX_TCP_CONNECTIONS} connections open"
            );
            continue; // dropping the stream closes it
        };
        let handler = handler.clone();
        connections.spawn(async move {
            tcp_connection(stream, src, &handler, idle).await;
            drop(permit);
        });
    }
}

/// Answer the requests on one TCP connection until the client closes it or
/// sends nothing for `idle`. As in hickory-server: a message whose header
/// cannot be read and a response message are ignored, a request that does
/// not decode gets FORMERR.
async fn tcp_connection(
    stream: tokio::net::TcpStream,
    src: SocketAddr,
    handler: &Handler,
    idle: Duration,
) {
    let (stream, stream_handle) =
        TcpStream::from_stream_with_buffer_size(AsyncIoTokioAsStd(stream), src, TCP_RESPONSE_QUEUE);
    let mut requests = TimeoutStream::new(stream, idle);
    while let Some(message) = requests.next().await {
        let bytes = match message {
            Ok(m) => m.into_parts().0,
            Err(e) => {
                tracing::debug!("DNS TCP connection from {src}: {e}");
                return;
            }
        };
        let mut respond = ResponseHandle::new(src, stream_handle.clone(), Protocol::Tcp);
        let Ok(header) = Header::read(&mut BinDecoder::new(&bytes)) else {
            continue;
        };
        if header.metadata.message_type == MessageType::Response {
            continue;
        }
        match Request::from_bytes(bytes, src, Protocol::Tcp) {
            Ok(request) => {
                handler
                    .handle_request::<_, TokioTime>(&request, respond)
                    .await;
            }
            Err(e) => {
                tracing::debug!("undecodable DNS request from {src}: {e}");
                let Ok(queries) = Queries::read(&mut BinDecoder::new(&[]), 0) else {
                    continue; // reading no question cannot fail
                };
                let response = MessageResponseBuilder::new(&queries, None)
                    .error_msg(&header.metadata, ResponseCode::FormErr);
                if let Err(e) = respond.send_response(response).await {
                    tracing::debug!("cannot send DNS response: {e}");
                }
            }
        }
    }
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
            slow_start_ms: 0,
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
                    backup: false,
                    recovered_at: None,
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
        let q = [(Name::from_ascii(name).unwrap(), t, DNSClass::IN)];
        answer(OpCode::Query, None, &q, v, &settings())
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
        use OnAllDown::{Closed, Open};
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
        let q = [(
            Name::from_ascii("api.svc.emfor").unwrap(),
            RecordType::A,
            DNSClass::IN,
        )];
        let picks: Vec<String> = (0..4)
            .map(|_| {
                answer(OpCode::Query, None, &q, &rr, &settings()).answers[0]
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
                answer(OpCode::Query, None, &q, &fo, &settings()).answers[0]
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

    /// The longest zone the config accepts still has a valid SOA.
    #[test]
    fn soa_for_the_longest_zone() {
        let zone = crate::config::tests::long_zone(crate::config::MAX_ZONE_LEN);
        let rec = soa_record(&DnsSettings {
            zone: zone.clone(),
            ..settings()
        });
        let RData::SOA(soa) = rec.data else {
            panic!("not SOA")
        };
        assert_eq!(soa.rname.to_ascii(), format!("hostmaster.{zone}."));
    }

    #[test]
    fn malformed_and_unsupported() {
        let v = LiveView::default();
        let s = settings();
        let q = (
            Name::from_ascii("svc.emfor").unwrap(),
            RecordType::SOA,
            DNSClass::IN,
        );
        assert_eq!(
            answer(OpCode::Update, None, std::slice::from_ref(&q), &v, &s).rcode,
            ResponseCode::NotImp
        );
        assert_eq!(
            answer(OpCode::Query, None, &[], &v, &s).rcode,
            ResponseCode::FormErr
        );
        assert_eq!(
            answer(OpCode::Query, None, &[q.clone(), q], &v, &s).rcode,
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
                    backup: false,
                    recovered_at: None,
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
    async fn live_server(v: LiveView) -> (DnsServer, SocketAddr, SocketAddr) {
        live_server_with(v, TCP_IDLE_TIMEOUT).await
    }

    async fn live_server_with(v: LiveView, idle: Duration) -> (DnsServer, SocketAddr, SocketAddr) {
        let live = LiveState::default();
        live.replace(v.services, v.site);
        let (_tx, rx) = watch::channel(settings());
        std::mem::forget(_tx);
        let listen = ["127.0.0.1:0".parse().unwrap()];
        let server = serve_with(&listen, Handler::new(live, rx), idle)
            .await
            .unwrap();
        let (ua, ta) = (server.udp[0].0, *server.tcp_addrs.values().next().unwrap());
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

    /// A query for `name` with class `class`, and EDNS `(payload, version)`.
    fn query_bytes(name: &str, t: RecordType, class: DNSClass, edns: Option<(u16, u8)>) -> Vec<u8> {
        let mut m = Message::query();
        m.metadata.id = 9;
        let mut q = Query::query(Name::from_ascii(name).unwrap(), t);
        q.set_query_class(class);
        m.add_query(q);
        if let Some((size, version)) = edns {
            let mut e = Edns::new();
            e.set_max_payload(size);
            e.set_version(version);
            m.set_edns(e);
        }
        m.to_vec().unwrap()
    }

    fn api_view() -> LiveView {
        view(vec![service(
            "api.svc.emfor",
            &[("10.0.0.11", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )])
    }

    /// Only class IN has a zone: CH, HS and the rest are REFUSED, like a
    /// name outside the zone.
    #[tokio::test]
    async fn class_other_than_in_is_refused() {
        let (_s, udp, _) = live_server(api_view()).await;
        let q = query_bytes("api.svc.emfor.", RecordType::A, DNSClass::IN, None);
        let (_, m) = over_udp(udp, &q).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NoError);
        assert_eq!(m.answers.len(), 1);
        for class in [DNSClass::CH, DNSClass::HS, DNSClass::NONE, DNSClass::ANY] {
            let q = query_bytes("api.svc.emfor.", RecordType::A, class, None);
            let (_, m) = over_udp(udp, &q).await;
            assert_eq!(m.metadata.response_code, ResponseCode::Refused, "{class}");
            assert!(m.answers.is_empty(), "{class}");
            assert!(!m.metadata.authoritative, "{class}");
        }
    }

    /// RFC 6891 6.1.3: an EDNS version above 0 gets BADVERS with an OPT
    /// record of version 0 and no answer.
    #[tokio::test]
    async fn edns_version_above_zero_is_badvers() {
        let (_s, udp, tcp) = live_server(api_view()).await;
        let q = query_bytes(
            "api.svc.emfor.",
            RecordType::A,
            DNSClass::IN,
            Some((1232, 1)),
        );
        let (_, m) = over_udp(udp, &q).await;
        // 16 is BADVERS; hickory decodes it as BADSIG, which shares it.
        assert_eq!(u16::from(m.metadata.response_code), 16, "BADVERS");
        assert!(m.answers.is_empty());
        assert_eq!(m.edns.as_ref().map(Edns::version), Some(0));
        let m = over_tcp(tcp, &q).await;
        assert_eq!(u16::from(m.metadata.response_code), 16, "BADVERS");
        // Version 0 is answered.
        let q = query_bytes(
            "api.svc.emfor.",
            RecordType::A,
            DNSClass::IN,
            Some((1232, 0)),
        );
        let (_, m) = over_udp(udp, &q).await;
        assert_eq!(m.metadata.response_code, ResponseCode::NoError);
        assert_eq!(m.answers.len(), 1);
    }

    /// Names are compared label by label: a label holding a `.` byte does
    /// not make a name part of the zone, and inside the zone it never
    /// matches a service, an SRV owner or a target name.
    #[test]
    fn label_with_a_dot_byte() {
        let v = view(vec![srv_service(
            "api.svc.emfor",
            &[("10.0.0.11:8080", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let nx = (ResponseCode::NXDomain, Vec::<String>::new(), true);
        let refused = (ResponseCode::Refused, Vec::<String>::new(), false);
        let rows: &[(&str, RecordType, Summary)] = &[
            // Labels `x.svc` and `emfor`: under `emfor`, not `svc.emfor`.
            ("x\\.svc.emfor", RecordType::A, refused.clone()),
            ("svc\\.emfor", RecordType::A, refused.clone()),
            // One label `_http._tcp` under the service.
            ("_http\\._tcp.api.svc.emfor", RecordType::SRV, nx.clone()),
            // One label `10-0-0-11.api` under the zone.
            ("10-0-0-11\\.api.svc.emfor", RecordType::A, nx.clone()),
            ("api\\.svc.emfor.svc.emfor", RecordType::A, nx.clone()),
        ];
        for (name, t, want) in rows {
            assert_eq!(&summary(&ask(&v, name, *t)), want, "{name} {t}");
        }
    }

    /// A service name of 252 characters: one A record does not fit in 512
    /// bytes, because hickory does not compress the owner name against the
    /// question. `fit_response` trims it (and keeps the OPT record).
    #[test]
    fn one_answer_can_overflow_512() {
        let name = format!(
            "{}.{}.{}.{}.svc.emfor",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(50)
        );
        assert_eq!(name.len(), 252);
        let v = view(vec![service(
            &name,
            &[("10.0.0.11", Healthy)],
            OnAllDown::Open,
            Balance::RoundRobin,
            None,
        )]);
        let queries = |t| {
            use hickory_proto::serialize::binary::{BinDecoder, BinEncodable};
            let q = Query::query(Name::from_ascii(format!("{name}.")).unwrap(), t);
            let bytes = q.to_bytes().unwrap();
            Queries::read(&mut BinDecoder::new(&bytes), 1).unwrap()
        };
        let mut e = Edns::new();
        e.set_max_payload(512);
        let mut r = ask(&v, &name, RecordType::A);
        assert_eq!(r.answers.len(), 1);
        assert!(encoded_len(&r, &queries(RecordType::A), Some(&e)) > 512);
        assert!(fit_response(&mut r, &queries(RecordType::A), Some(&e), 512));
        assert!(r.answers.is_empty());
        assert!(encoded_len(&r, &queries(RecordType::A), Some(&e)) <= 512);
        // NODATA: question and SOA fit.
        let mut r = ask(&v, &name, RecordType::TXT);
        assert!(!fit_response(&mut r, &queries(RecordType::TXT), None, 512));
        assert!(r.soa.is_some());
    }

    /// The longest zone: question and SOA alone exceed 512 bytes, so the
    /// SOA is left out and TC set.
    #[test]
    fn soa_dropped_when_it_does_not_fit() {
        let zone = crate::config::tests::long_zone(crate::config::MAX_ZONE_LEN);
        let s = DnsSettings {
            zone: zone.clone(),
            ..settings()
        };
        let q = [(
            Name::from_ascii(&zone).unwrap(),
            RecordType::TXT,
            DNSClass::IN,
        )];
        let ready = LiveView {
            ready: true,
            ..LiveView::default()
        };
        let mut r = answer(OpCode::Query, None, &q, &ready, &s);
        assert!(r.soa.is_some());
        let queries = {
            use hickory_proto::serialize::binary::{BinDecoder, BinEncodable};
            let q = Query::query(
                Name::from_ascii(format!("{zone}.")).unwrap(),
                RecordType::TXT,
            );
            let bytes = q.to_bytes().unwrap();
            Queries::read(&mut BinDecoder::new(&bytes), 1).unwrap()
        };
        assert!(fit_response(&mut r, &queries, None, 512));
        assert!(r.soa.is_none());
        assert!(encoded_len(&r, &queries, None) <= 512);
    }

    /// Open `n` TCP connections to `addr` and let the server accept them.
    async fn connections(addr: SocketAddr, n: usize) -> Vec<tokio::net::TcpStream> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(tokio::net::TcpStream::connect(addr).await.unwrap());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        v
    }

    /// True when the server closes `s` (EOF or reset) within two seconds.
    async fn closed_by_server(s: &mut tokio::net::TcpStream) -> bool {
        use tokio::io::AsyncReadExt;
        let mut b = [0u8; 1];
        matches!(
            tokio::time::timeout(Duration::from_secs(2), s.read(&mut b)).await,
            Ok(Ok(0) | Err(_))
        )
    }

    /// One query over an open connection.
    async fn ask_on(s: &mut tokio::net::TcpStream, query: &[u8]) -> Message {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut framed = (query.len() as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(query);
        s.write_all(&framed).await.unwrap();
        let mut len = [0u8; 2];
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut len))
            .await
            .unwrap()
            .unwrap();
        let mut buf = vec![0u8; usize::from(u16::from_be_bytes(len))];
        s.read_exact(&mut buf).await.unwrap();
        Message::from_vec(&buf).unwrap()
    }

    /// More than MAX_TCP_CONNECTIONS at once: the extra one is closed right
    /// away, the open ones still answer, and once they close new ones are
    /// served again.
    #[tokio::test]
    async fn tcp_connection_limit() {
        let (_s, _, tcp) = live_server(api_view()).await;
        let mut open = connections(tcp, MAX_TCP_CONNECTIONS).await;
        let mut extra = tokio::net::TcpStream::connect(tcp).await.unwrap();
        assert!(
            closed_by_server(&mut extra).await,
            "extra connection kept open"
        );
        let q = query_bytes("api.svc.emfor.", RecordType::A, DNSClass::IN, None);
        for s in [0, MAX_TCP_CONNECTIONS - 1] {
            let m = ask_on(&mut open[s], &q).await;
            assert_eq!(m.answers.len(), 1, "connection {s}");
        }
        drop(open);
        for _ in 0..50 {
            let mut s = tokio::net::TcpStream::connect(tcp).await.unwrap();
            if !closed_by_server(&mut s).await {
                assert_eq!(ask_on(&mut s, &q).await.answers.len(), 1);
                return;
            }
        }
        panic!("no connection served after the others closed");
    }

    /// A connection that sends nothing for the idle timeout is closed; one
    /// that keeps asking is not.
    #[tokio::test]
    async fn tcp_idle_connection_closed() {
        let idle = Duration::from_millis(300);
        let (_s, _, tcp) = live_server_with(api_view(), idle).await;
        let q = query_bytes("api.svc.emfor.", RecordType::A, DNSClass::IN, None);
        let mut busy = tokio::net::TcpStream::connect(tcp).await.unwrap();
        let mut quiet = tokio::net::TcpStream::connect(tcp).await.unwrap();
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(ask_on(&mut busy, &q).await.answers.len(), 1);
        }
        assert!(closed_by_server(&mut quiet).await);
    }

    /// Requests that do not decode get FORMERR over TCP, as over UDP; the
    /// connection stays usable.
    #[tokio::test]
    async fn tcp_undecodable_request_is_formerr() {
        let (_s, _, tcp) = live_server(api_view()).await;
        let mut s = tokio::net::TcpStream::connect(tcp).await.unwrap();
        let mut bad = query_bytes("api.svc.emfor.", RecordType::A, DNSClass::IN, None);
        bad.truncate(bad.len() - 2);
        let m = ask_on(&mut s, &bad).await;
        assert_eq!(m.metadata.response_code, ResponseCode::FormErr);
        assert_eq!(m.metadata.id, 9);
        let q = query_bytes("api.svc.emfor.", RecordType::A, DNSClass::IN, None);
        assert_eq!(ask_on(&mut s, &q).await.answers.len(), 1);
    }

    /// `ended` completes when a listener stops, naming it, and stays
    /// pending while they run.
    #[tokio::test]
    async fn ended_reports_a_stopped_listener() {
        let (mut s, udp, tcp) = live_server(api_view()).await;
        let wait = tokio::time::timeout(Duration::from_millis(200), s.ended()).await;
        assert!(wait.is_err(), "ended while running");
        s.tcp.abort_all();
        let why = tokio::time::timeout(Duration::from_secs(2), s.ended())
            .await
            .unwrap();
        assert_eq!(why, format!("DNS listener on {tcp} (tcp) stopped"));
        s.udp[0].1.shutdown_token().cancel();
        let why = tokio::time::timeout(Duration::from_secs(2), s.ended())
            .await
            .unwrap();
        assert_eq!(why, format!("DNS listener on {udp} (udp) stopped"));
    }
}
