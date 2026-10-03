//! Bounded passive transaction matching with explicit UDS/KWP header profiles.
use crate::{
    core::Location,
    isotp::{Config as TransportConfig, Direction, Event, ResponseStart},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Uds2013,
    Kwp2000Vector,
}
impl Protocol {
    pub fn cdd_label(self) -> &'static str {
        match self {
            Self::Uds2013 => "uds",
            Self::Kwp2000Vector => "kwp2000",
        }
    }
    fn is_uds(&self) -> bool {
        *self == Self::Uds2013
    }
    fn kind(self, status: Status, issue: bool) -> &'static str {
        match (self, status, issue) {
            (Self::Uds2013, _, true) => "uds_issue",
            (Self::Uds2013, Status::Pending, false) => "uds_pending",
            (Self::Uds2013, _, false) => "uds_transaction",
            (Self::Kwp2000Vector, _, true) => "kwp_issue",
            (Self::Kwp2000Vector, Status::Pending, false) => "kwp_pending",
            (Self::Kwp2000Vector, _, false) => "kwp_transaction",
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutePolicy {
    pub route: String,
    pub protocol: Protocol,
    pub p2_ns: i64,
    pub p2_star_ns: i64,
    pub transaction_max_duration_ns: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdd: Option<crate::cdd::Assignment>,
}
fn outstanding_limit() -> usize {
    64
}
fn request_limit() -> usize {
    262_144
}
fn example_limit() -> usize {
    16
}
fn pending_limit() -> u64 {
    256
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub routes: Vec<RoutePolicy>,
    #[serde(default = "outstanding_limit")]
    pub max_outstanding: usize,
    #[serde(default = "request_limit")]
    pub max_total_request_bytes: usize,
    #[serde(default = "example_limit")]
    pub max_pending_examples: usize,
    #[serde(default = "pending_limit")]
    pub max_pending_per_transaction: u64,
}
impl Config {
    pub fn validate(&self, transport: &TransportConfig) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported diagnostic policy schema"
        );
        ensure!(
            (1..=64).contains(&self.routes.len()),
            "diagnostic policy requires 1..64 physical route bindings"
        );
        ensure!(
            (1..=128).contains(&self.max_outstanding),
            "max_outstanding must be 1..128"
        );
        ensure!(
            (1..=1_048_576).contains(&self.max_total_request_bytes),
            "max_total_request_bytes must be 1..1048576"
        );
        ensure!(
            self.max_pending_examples <= 64,
            "max_pending_examples must be 0..64"
        );
        ensure!(
            (1..=65536).contains(&self.max_pending_per_transaction),
            "max_pending_per_transaction must be 1..65536"
        );
        let mut names = BTreeSet::new();
        for policy in &self.routes {
            ensure!(
                names.insert(&policy.route)
                    && transport.routes.iter().any(|r| r.name == policy.route),
                "diagnostic route bindings must be unique and refer to existing transport routes"
            );
            ensure!(
                (1..=60_000_000_000).contains(&policy.p2_ns)
                    && (1..=60_000_000_000).contains(&policy.p2_star_ns),
                "P2/P2* must be 1..60000000000 ns"
            );
            ensure!(
                (1..=600_000_000_000).contains(&policy.transaction_max_duration_ns)
                    && policy.p2_ns <= policy.transaction_max_duration_ns
                    && policy.p2_star_ns <= policy.transaction_max_duration_ns,
                "transaction_max_duration_ns must cover P2/P2* and be at most 600 seconds"
            );
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Payload {
    pub data_hex: String,
    pub first_timestamp_ns: i64,
    pub last_timestamp_ns: i64,
    pub first_location: Location,
    pub last_location: Location,
    pub data_locations: Vec<Location>,
    pub completeness: String,
    pub fc_observation: String,
    pub capture_gaps_before: u64,
}
impl Payload {
    fn of(event: &Event) -> Result<Self> {
        let first = event
            .first_timestamp_ns
            .ok_or_else(|| anyhow::anyhow!("transport payload time is unknown"))?;
        let last = event
            .last_timestamp_ns
            .ok_or_else(|| anyhow::anyhow!("transport payload time is unknown"))?;
        ensure!(
            first >= 0 && last >= first,
            "invalid transport payload clock"
        );
        Ok(Self {
            data_hex: event.data_hex.clone(),
            first_timestamp_ns: first,
            last_timestamp_ns: last,
            first_location: event.first_location.clone(),
            last_location: event.last_location.clone(),
            data_locations: event.data_locations.clone(),
            completeness: event.status.clone(),
            fc_observation: event.fc_observation.clone(),
            capture_gaps_before: event.capture_gaps_before,
        })
    }
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestHeader {
    pub service_id: u8,
    pub subfunction: Option<u8>,
    pub identifiers: Vec<u16>,
    pub routine_id: Option<u16>,
    pub block_sequence_counter: Option<u8>,
    pub suppress_positive_response: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_mode: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_mode: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_identifier: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dtc_group: Option<u16>,
}
fn supported(sid: u8) -> bool {
    matches!(
        sid,
        0x10 | 0x11 | 0x14 | 0x19 | 0x22 | 0x27 | 0x2e | 0x31 | 0x34 | 0x36 | 0x37 | 0x3e
    )
}
fn word(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}
fn request_header(bytes: &[u8]) -> std::result::Result<RequestHeader, &'static str> {
    let sid = *bytes.first().ok_or("malformed_empty_request")?;
    if !supported(sid) {
        return Err("unsupported_service_profile");
    }
    let mut header = RequestHeader {
        service_id: sid,
        subfunction: None,
        identifiers: vec![],
        routine_id: None,
        block_sequence_counter: None,
        suppress_positive_response: false,
        ..Default::default()
    };
    if matches!(sid, 0x10 | 0x11 | 0x19 | 0x27 | 0x31 | 0x3e) {
        if bytes.len() < 2 {
            return Err("malformed_request_subfunction");
        }
        header.subfunction = Some(bytes[1] & 0x7f);
        header.suppress_positive_response = bytes[1] & 0x80 != 0;
    }
    let valid = match sid {
        0x10 | 0x11 => bytes.len() == 2 && header.subfunction != Some(0),
        0x14 => bytes.len() == 4,
        0x19 => {
            if !matches!(header.subfunction, Some(1 | 2)) {
                return Err("unsupported_dtc_subfunction_profile");
            }
            bytes.len() == 3
        }
        0x22 => {
            if bytes.len() < 3 || bytes.len() % 2 != 1 {
                false
            } else {
                header.identifiers = bytes[1..]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| word(pair))
                    .collect();
                true
            }
        }
        0x27 => bytes.len() >= 2 && header.subfunction.is_some_and(|n| n > 0 && n < 0x7f),
        0x2e => {
            if bytes.len() < 4 {
                false
            } else {
                header.identifiers.push(word(&bytes[1..3]));
                true
            }
        }
        0x31 => {
            if bytes.len() < 4 || !matches!(header.subfunction, Some(1..=3)) {
                false
            } else {
                header.routine_id = Some(word(&bytes[2..4]));
                true
            }
        }
        0x34 => {
            if bytes.len() < 3 {
                false
            } else {
                let address = usize::from(bytes[2] & 15);
                let size = usize::from(bytes[2] >> 4);
                (1..=8).contains(&address)
                    && (1..=8).contains(&size)
                    && bytes.len() == 3 + address + size
            }
        }
        0x36 => {
            if bytes.len() < 2 {
                false
            } else {
                header.block_sequence_counter = Some(bytes[1]);
                true
            }
        }
        0x37 => true,
        0x3e => bytes.len() == 2 && header.subfunction == Some(0),
        _ => false,
    };
    if valid {
        Ok(header)
    } else {
        Err("malformed_request_header")
    }
}
pub(crate) struct ResponseHeader {
    pub sid: u8,
    pub nrc: Option<u8>,
}
fn response_header(bytes: &[u8]) -> std::result::Result<ResponseHeader, &'static str> {
    let first = *bytes.first().ok_or("malformed_empty_response")?;
    if first == 0x7f {
        if bytes.len() != 3 || bytes[2] == 0 {
            return Err("malformed_negative_response");
        }
        if !supported(bytes[1]) {
            return Err("unsupported_negative_service_profile");
        }
        return Ok(ResponseHeader {
            sid: bytes[1],
            nrc: Some(bytes[2]),
        });
    }
    let sid = first
        .checked_sub(0x40)
        .ok_or("unsupported_response_profile")?;
    if !supported(sid) {
        return Err("unsupported_response_profile");
    }
    if matches!(sid, 0x10 | 0x11 | 0x19 | 0x27 | 0x31 | 0x3e)
        && (bytes.len() < 2 || bytes[1] & 0x80 != 0)
    {
        return Err("malformed_response_subfunction");
    }
    let valid = match sid {
        0x10 => bytes.len() == 6,
        0x11 => {
            if bytes[1] == 4 {
                bytes.len() == 3
            } else {
                bytes.len() == 2
            }
        }
        0x14 => bytes.len() == 1,
        0x19 => match bytes[1] {
            1 => bytes.len() == 6,
            2 => bytes.len() >= 3 && (bytes.len() - 3).is_multiple_of(4),
            _ => return Err("unsupported_dtc_response_profile"),
        },
        0x22 => bytes.len() >= 3,
        0x27 => bytes.len() >= 2,
        0x2e => bytes.len() == 3,
        0x31 => bytes.len() >= 4,
        0x34 => {
            bytes.len() >= 3
                && bytes[1] & 15 == 0
                && (1..=8).contains(&(bytes[1] >> 4))
                && bytes.len() == 2 + usize::from(bytes[1] >> 4)
        }
        0x36 => bytes.len() >= 2,
        0x37 => true,
        0x3e => bytes.len() == 2 && bytes[1] == 0,
        _ => false,
    };
    if valid {
        Ok(ResponseHeader { sid, nrc: None })
    } else {
        Err("malformed_response_header")
    }
}
#[derive(PartialEq, Eq)]
enum Match {
    No,
    Yes,
    Unverified,
}
fn matches(header: &RequestHeader, response: &ResponseHeader, data: &[u8]) -> Match {
    if header.service_id != response.sid {
        return Match::No;
    }
    if response.nrc.is_some() {
        return Match::Yes;
    }
    let valid = match header.service_id {
        0x10 | 0x11 | 0x19 | 0x27 | 0x3e => header.subfunction == Some(data[1]),
        0x31 => header.subfunction == Some(data[1]) && header.routine_id == Some(word(&data[2..4])),
        0x22 => {
            let did = word(&data[1..3]);
            if header.identifiers.len() > 1 && header.identifiers.contains(&did) {
                return Match::Unverified;
            }
            header.identifiers.first() == Some(&did)
        }
        0x2e => header.identifiers.first() == Some(&word(&data[1..3])),
        0x36 => header.block_sequence_counter == Some(data[1]),
        0x14 | 0x34 | 0x37 => true,
        _ => false,
    };
    if valid {
        Match::Yes
    } else {
        Match::No
    }
}
fn bytes(event: &Event) -> Result<Vec<u8>> {
    let text = event.data_hex.as_bytes();
    ensure!(
        text.len() <= 8190 && text.len().is_multiple_of(2),
        "invalid transport payload hex length"
    );
    let mut result = Vec::with_capacity(text.len() / 2);
    fn digit(byte: u8) -> Result<u8> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => anyhow::bail!("invalid transport hex"),
        }
    }
    for pair in text.as_chunks::<2>().0 {
        result.push((digit(pair[0])? << 4) | digit(pair[1])?);
    }
    ensure!(
        result.len() == event.observed_length,
        "transport observed length mismatch"
    );
    Ok(result)
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Positive,
    Negative,
    Pending,
    PendingOnly,
    NoResponseObserved,
    SuppressedExpected,
    Ambiguous,
    Incomplete,
    Orphan,
    Malformed,
    Unsupported,
    ResourceLimit,
}
impl Status {
    pub fn complete_analysis(self) -> bool {
        matches!(
            self,
            Self::Positive | Self::Negative | Self::Pending | Self::SuppressedExpected
        )
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Protocol::is_uds")]
    pub protocol: Protocol,
    pub route: String,
    pub scope: &'static str,
    pub lifecycle: &'static str,
    pub status: Status,
    pub reason: Option<String>,
    pub transaction_key: Option<String>,
    pub candidate_transaction_keys: Vec<String>,
    pub header: Option<RequestHeader>,
    pub request: Option<Payload>,
    pub response: Option<Payload>,
    pub response_match: &'static str,
    pub nrc: Option<u8>,
    pub pending_count: u64,
    pub pending_examples: Vec<Payload>,
    pub omitted_pending_examples: u64,
    pub request_last_to_response_first_ns: Option<i64>,
    pub request_first_to_response_last_ns: Option<i64>,
    pub observed_at_timestamp_ns: i64,
    pub p2_deadline_ns: Option<i64>,
    pub total_deadline_ns: Option<i64>,
    pub data_semantics: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdd: Option<crate::cdd::Decoded>,
}
impl Observation {
    fn issue(
        protocol: Protocol,
        route: &str,
        event: &Event,
        status: Status,
        reason: &str,
    ) -> Result<Self> {
        Ok(Self {
            kind: protocol.kind(status, true),
            protocol,
            route: route.into(),
            scope: "physical",
            lifecycle: "closed",
            status,
            reason: Some(reason.into()),
            transaction_key: None,
            candidate_transaction_keys: vec![],
            header: None,
            request: if event.direction == Some(Direction::Request) {
                Some(Payload::of(event)?)
            } else {
                None
            },
            response: if event.direction == Some(Direction::Response) {
                Some(Payload::of(event)?)
            } else {
                None
            },
            response_match: "unmatched",
            nrc: None,
            pending_count: 0,
            pending_examples: vec![],
            omitted_pending_examples: 0,
            request_last_to_response_first_ns: None,
            request_first_to_response_last_ns: None,
            observed_at_timestamp_ns: event.last_timestamp_ns.unwrap_or(0),
            p2_deadline_ns: None,
            total_deadline_ns: None,
            data_semantics: "opaque_no_cdd",
            cdd: None,
        })
    }
}
struct Transaction {
    key: String,
    protocol: Protocol,
    binding: usize,
    header: RequestHeader,
    request: Payload,
    byte_count: usize,
    deadline: i64,
    total_deadline: i64,
    pending_count: u64,
    pending: Vec<Payload>,
}
impl Transaction {
    fn observation(
        &self,
        route: &str,
        status: Status,
        reason: Option<&str>,
        response: Option<Payload>,
        now: i64,
    ) -> Observation {
        let matched = matches!(
            status,
            Status::Positive | Status::Negative | Status::Pending
        );
        Observation {
            kind: self.protocol.kind(status, false),
            protocol: self.protocol,
            route: route.into(),
            scope: "physical",
            lifecycle: if status == Status::Pending {
                "open"
            } else {
                "closed"
            },
            status,
            reason: reason.map(str::to_owned),
            transaction_key: Some(self.key.clone()),
            candidate_transaction_keys: vec![],
            header: Some(self.header.clone()),
            request: Some(self.request.clone()),
            request_last_to_response_first_ns: response
                .as_ref()
                .filter(|_| matched)
                .map(|p| p.first_timestamp_ns - self.request.last_timestamp_ns),
            request_first_to_response_last_ns: response
                .as_ref()
                .filter(|_| matched)
                .map(|p| p.last_timestamp_ns - self.request.first_timestamp_ns),
            response,
            response_match: if matched { "matched" } else { "unverified" },
            nrc: None,
            pending_count: self.pending_count,
            pending_examples: self.pending.clone(),
            omitted_pending_examples: self.pending_count - self.pending.len() as u64,
            observed_at_timestamp_ns: now,
            p2_deadline_ns: Some(self.deadline),
            total_deadline_ns: Some(self.total_deadline),
            data_semantics: "opaque_no_cdd",
            cdd: None,
        }
    }
}
pub struct Matcher {
    config: Config,
    channels: Vec<u16>,
    timeline: String,
    open: Vec<Transaction>,
    reserved: usize,
}
impl Matcher {
    pub fn new(config: Config, transport: &TransportConfig, timeline: &str) -> Result<Self> {
        transport.validate()?;
        config.validate(transport)?;
        let channels = config
            .routes
            .iter()
            .map(|p| {
                transport
                    .routes
                    .iter()
                    .find(|r| r.name == p.route)
                    .unwrap()
                    .channel
            })
            .collect();
        Ok(Self {
            config,
            channels,
            timeline: timeline.into(),
            open: vec![],
            reserved: 0,
        })
    }
    pub fn outstanding(&self) -> usize {
        self.open.len()
    }
    pub fn reserved_request_bytes(&self) -> usize {
        self.reserved
    }
    fn close(
        &mut self,
        index: usize,
        status: Status,
        reason: &str,
        response: Option<Payload>,
        now: i64,
    ) -> Observation {
        let transaction = self.open.remove(index);
        self.reserved -= transaction.byte_count;
        transaction.observation(
            &self.config.routes[transaction.binding].route,
            status,
            Some(reason),
            response,
            now,
        )
    }
    fn interrupt_route(
        &mut self,
        route: &str,
        reason: &str,
        event: Option<&Event>,
        now: i64,
    ) -> Result<Vec<Observation>> {
        let mut result = vec![];
        for index in (0..self.open.len()).rev() {
            if self.config.routes[self.open[index].binding].route == route {
                let response = event
                    .filter(|e| e.direction == Some(Direction::Response) && e.kind == "payload")
                    .map(Payload::of)
                    .transpose()?;
                result.push(self.close(index, Status::Incomplete, reason, response, now));
            }
        }
        result.reverse();
        Ok(result)
    }
    /// Completed payloads alone enter the explicit protocol classifier; transport gaps close unresolved context.
    pub fn consume(&mut self, event: &Event) -> Result<Vec<Observation>> {
        let Some(route) = event.route.as_deref() else {
            return Ok(vec![]);
        };
        let Some(binding) = self.config.routes.iter().position(|p| p.route == route) else {
            return Ok(vec![]);
        };
        let protocol = self.config.routes[binding].protocol;
        if event.kind == "protocol_issue" || (event.kind == "payload" && event.status != "complete")
        {
            return self.interrupt_route(
                route,
                event.reason.as_deref().unwrap_or("incomplete_transport"),
                Some(event),
                event.last_timestamp_ns.unwrap_or(0),
            );
        }
        if event.kind != "payload" {
            return Ok(vec![]);
        };
        let data = bytes(event)?;
        let payload = Payload::of(event)?;
        ensure!(
            event.declared_length == Some(data.len()),
            "completed transport declared length mismatch"
        );
        if event.direction == Some(Direction::Request) {
            let classified = match protocol {
                Protocol::Uds2013 => request_header(&data),
                Protocol::Kwp2000Vector => crate::kwp::request_header(&data),
            };
            let header = match classified {
                Ok(header) => header,
                Err(reason) => {
                    let mut result = self.interrupt_route(
                        route,
                        "unverified_request_context",
                        None,
                        payload.last_timestamp_ns,
                    )?;
                    result.push(Observation::issue(
                        protocol,
                        route,
                        event,
                        if reason.starts_with("unsupported") {
                            Status::Unsupported
                        } else {
                            Status::Malformed
                        },
                        reason,
                    )?);
                    return Ok(result);
                }
            };
            if self.open.len() >= self.config.max_outstanding
                || self.reserved + data.len() > self.config.max_total_request_bytes
            {
                let mut result = self.interrupt_route(
                    route,
                    "request_resource_limit_context_gap",
                    None,
                    payload.last_timestamp_ns,
                )?;
                result.push(Observation::issue(
                    protocol,
                    route,
                    event,
                    Status::ResourceLimit,
                    "outstanding_or_request_buffer_limit",
                )?);
                return Ok(result);
            }
            let policy = &self.config.routes[binding];
            let deadline = payload
                .last_timestamp_ns
                .checked_add(policy.p2_ns)
                .ok_or_else(|| anyhow::anyhow!("diagnostic P2 deadline overflow"))?;
            let total_deadline = payload
                .last_timestamp_ns
                .checked_add(policy.transaction_max_duration_ns)
                .ok_or_else(|| anyhow::anyhow!("diagnostic total deadline overflow"))?;
            let key = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&(
                    &self.timeline,
                    route,
                    &payload.first_location,
                    &data
                ))?)
            );
            self.reserved += data.len();
            self.open.push(Transaction {
                key,
                protocol,
                binding,
                header,
                request: payload,
                byte_count: data.len(),
                deadline,
                total_deadline,
                pending_count: 0,
                pending: vec![],
            });
            return Ok(vec![]);
        }
        let classified = match protocol {
            Protocol::Uds2013 => response_header(&data),
            Protocol::Kwp2000Vector => crate::kwp::response_header(&data),
        };
        let response = match classified {
            Ok(header) => header,
            Err(reason) => {
                return Ok(vec![Observation::issue(
                    protocol,
                    route,
                    event,
                    if reason.starts_with("unsupported") {
                        Status::Unsupported
                    } else {
                        Status::Malformed
                    },
                    reason,
                )?])
            }
        };
        let candidates: Vec<(usize, Match)> = self
            .open
            .iter()
            .enumerate()
            .filter_map(|(index, t)| {
                if t.binding != binding
                    || payload.first_timestamp_ns < t.request.last_timestamp_ns
                    || (payload.first_timestamp_ns == t.request.last_timestamp_ns
                        && payload.first_location.ordinal <= t.request.last_location.ordinal)
                    || payload.first_timestamp_ns > t.deadline.min(t.total_deadline)
                    || payload.last_timestamp_ns > t.total_deadline
                {
                    return None;
                }
                let result = match protocol {
                    Protocol::Uds2013 => matches(&t.header, &response, &data),
                    Protocol::Kwp2000Vector => {
                        if crate::kwp::matches(&t.header, &response, &data) {
                            Match::Yes
                        } else {
                            Match::No
                        }
                    }
                };
                (result != Match::No).then_some((index, result))
            })
            .collect();
        if candidates.is_empty() {
            let mut issue = Observation::issue(
                protocol,
                route,
                event,
                Status::Orphan,
                "no_unique_timely_request_context",
            )?;
            issue.nrc = response.nrc;
            return Ok(vec![issue]);
        }
        if candidates.len() > 1 || candidates[0].1 == Match::Unverified {
            let keys: Vec<String> = candidates
                .iter()
                .map(|(index, _)| self.open[*index].key.clone())
                .collect();
            let reason = if candidates.len() > 1 {
                "multiple_request_candidates"
            } else {
                "multi_did_response_requires_definition"
            };
            let mut result = vec![];
            for (index, _) in candidates.into_iter().rev() {
                let mut closed = self.close(
                    index,
                    Status::Ambiguous,
                    reason,
                    Some(payload.clone()),
                    payload.last_timestamp_ns,
                );
                closed.candidate_transaction_keys = keys.clone();
                closed.nrc = response.nrc;
                result.push(closed);
            }
            result.reverse();
            return Ok(result);
        }
        let index = candidates[0].0;
        if response.nrc == Some(0x78) {
            let transaction = &mut self.open[index];
            transaction.pending_count += 1;
            if transaction.pending.len() < self.config.max_pending_examples {
                transaction.pending.push(payload.clone());
            }
            if transaction.pending_count >= self.config.max_pending_per_transaction {
                let mut closed = self.close(
                    index,
                    Status::PendingOnly,
                    "pending_count_observation_limit",
                    Some(payload.clone()),
                    payload.last_timestamp_ns,
                );
                closed.nrc = Some(0x78);
                return Ok(vec![closed]);
            }
            transaction.deadline = payload
                .last_timestamp_ns
                .checked_add(self.config.routes[binding].p2_star_ns)
                .ok_or_else(|| anyhow::anyhow!("diagnostic P2* deadline overflow"))?
                .min(transaction.total_deadline);
            let mut observed = transaction.observation(
                route,
                Status::Pending,
                None,
                Some(payload.clone()),
                payload.last_timestamp_ns,
            );
            observed.nrc = Some(0x78);
            return Ok(vec![observed]);
        }
        let transaction = self.open.remove(index);
        self.reserved -= transaction.byte_count;
        let mut observed = transaction.observation(
            route,
            if response.nrc.is_some() {
                Status::Negative
            } else {
                Status::Positive
            },
            None,
            Some(payload.clone()),
            payload.last_timestamp_ns,
        );
        observed.nrc = response.nrc;
        Ok(vec![observed])
    }
    /// A timely unresolved ISO-TP response start holds P2/P2*, bounded by total duration.
    pub fn advance(&mut self, now: i64, receiving: &[ResponseStart]) -> Vec<Observation> {
        let mut result = vec![];
        for index in (0..self.open.len()).rev() {
            let transaction = &self.open[index];
            let route = &self.config.routes[transaction.binding].route;
            let started = receiving.iter().any(|p| {
                p.route == *route
                    && p.first_timestamp_ns >= transaction.request.last_timestamp_ns
                    && (p.first_timestamp_ns != transaction.request.last_timestamp_ns
                        || p.first_ordinal > transaction.request.last_location.ordinal)
                    && p.first_timestamp_ns <= transaction.deadline.min(transaction.total_deadline)
            });
            if now <= transaction.deadline.min(transaction.total_deadline) {
                continue;
            }
            if started && now <= transaction.total_deadline {
                continue;
            }
            let reason = if now > transaction.total_deadline {
                "observation_limit"
            } else {
                "response_deadline_observed_in_log"
            };
            let status = if started {
                Status::Incomplete
            } else if transaction.pending_count > 0 {
                Status::PendingOnly
            } else if transaction.header.suppress_positive_response {
                Status::SuppressedExpected
            } else {
                Status::NoResponseObserved
            };
            result.push(self.close(index, status, reason, None, now));
        }
        result.reverse();
        result
    }
    pub fn gap(&mut self, channel: Option<u16>, now: i64) -> Vec<Observation> {
        let mut result = vec![];
        for index in (0..self.open.len()).rev() {
            if channel.is_none_or(|c| c == self.channels[self.open[index].binding]) {
                result.push(self.close(index, Status::Incomplete, "capture_gap", None, now));
            }
        }
        result.reverse();
        result
    }
    pub fn eof(&mut self, now: i64) -> Vec<Observation> {
        let mut result = vec![];
        while !self.open.is_empty() {
            result.push(self.close(
                0,
                Status::Incomplete,
                "end_of_file_observation_incomplete",
                None,
                now,
            ));
        }
        result
    }
}
