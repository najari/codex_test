//! Bounded, passive Classic CAN ISO-TP reconstruction on one ordered log clock.
use crate::core::{FrameRecord, Location};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Addressing {
    Normal,
    Extended,
    Mixed,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    Physical,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Classic,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub id: u32,
    pub extended: bool,
    #[serde(default)]
    pub address: Option<u8>,
}
fn timeout() -> i64 {
    1_000_000_000
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub name: String,
    pub channel: u16,
    pub kind: RouteKind,
    pub profile: Profile,
    pub addressing: Addressing,
    pub request: Endpoint,
    pub response: Endpoint,
    /// Optional validation of bytes after an SF/final CF/FC; padding length is not required.
    #[serde(default)]
    pub padding_byte: Option<u8>,
    #[serde(default = "timeout")]
    pub timeout_ns: i64,
}
fn payload_limit() -> usize {
    4095
}
fn buffer_limit() -> usize {
    262_144
}
fn session_limit() -> usize {
    64
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub routes: Vec<Route>,
    #[serde(default = "payload_limit")]
    pub max_payload_bytes: usize,
    #[serde(default = "buffer_limit")]
    pub max_total_payload_bytes: usize,
    #[serde(default = "session_limit")]
    pub max_sessions: usize,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported ISO-TP route schema");
        ensure!(
            (1..=64).contains(&self.routes.len()),
            "routes must contain 1..64 physical routes"
        );
        ensure!(
            (1..=4095).contains(&self.max_payload_bytes),
            "max_payload_bytes must be 1..4095 for classic profile"
        );
        ensure!(
            (1..=1_048_576).contains(&self.max_total_payload_bytes),
            "max_total_payload_bytes must be 1..1048576"
        );
        ensure!(
            (1..=128).contains(&self.max_sessions),
            "max_sessions must be 1..128"
        );
        let mut names = BTreeSet::new();
        let mut endpoints: Vec<(u16, &Endpoint)> = Vec::new();
        for route in &self.routes {
            ensure!(
                !route.name.is_empty() && route.name.len() <= 128 && names.insert(&route.name),
                "route names must be unique and 1..128 bytes"
            );
            ensure!(route.channel > 0, "route channel must be nonzero");
            ensure!(
                (1..=60_000_000_000).contains(&route.timeout_ns),
                "timeout_ns must be 1..60000000000"
            );
            ensure!(
                (route.request.id, route.request.extended)
                    != (route.response.id, route.response.extended),
                "physical endpoints must have distinct CAN IDs/kinds"
            );
            for endpoint in [&route.request, &route.response] {
                ensure!(
                    endpoint.id
                        <= if endpoint.extended {
                            0x1fff_ffff
                        } else {
                            0x7ff
                        },
                    "route CAN ID outside its declared range"
                );
                ensure!(
                    (route.addressing == Addressing::Normal) == endpoint.address.is_none(),
                    "normal routes omit address bytes; extended/mixed routes require them"
                );
                for (channel, other) in &endpoints {
                    ensure!(
                        *channel != route.channel
                            || other.id != endpoint.id
                            || other.extended != endpoint.extended
                            || (other.address.is_some()
                                && endpoint.address.is_some()
                                && other.address != endpoint.address),
                        "ambiguous route endpoint on channel {} ID {}",
                        route.channel,
                        endpoint.id
                    );
                }
                endpoints.push((route.channel, endpoint));
            }
            if route.addressing == Addressing::Mixed {
                ensure!(
                    route.request.address == route.response.address,
                    "mixed address bytes must match"
                );
            }
            if route.addressing == Addressing::Extended {
                ensure!(
                    route.request.address != route.response.address,
                    "extended address bytes must differ"
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Request,
    Response,
}
impl Direction {
    fn index(self) -> usize {
        match self {
            Self::Request => 0,
            Self::Response => 1,
        }
    }
    fn opposite(self) -> Self {
        match self {
            Self::Request => Self::Response,
            Self::Response => Self::Request,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FlowObservation {
    pub count: u64,
    pub cts: u64,
    pub wait: u64,
    pub overflow: u64,
    pub reserved_stmin: u64,
    pub last_block_size: Option<u8>,
    pub last_stmin_byte: Option<u8>,
    pub last_stmin_ns: Option<i64>,
    pub minimum_cf_delta_ns: Option<i64>,
    /// Observations only: clock precision/capture loss can prevent a compliance verdict.
    pub shorter_than_stmin: u64,
    pub block_size_exceeded: u64,
    pub cf_after_wait: u64,
    pub locations: Vec<Location>,
    pub omitted_locations: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub kind: String,
    pub route: Option<String>,
    pub direction: Option<Direction>,
    pub status: String,
    pub reason: Option<String>,
    pub first_timestamp_ns: Option<i64>,
    pub last_timestamp_ns: Option<i64>,
    pub first_location: Location,
    pub last_location: Location,
    pub declared_length: Option<usize>,
    pub observed_length: usize,
    /// Includes only the verified prefix for incomplete/aborted sessions.
    pub data_hex: String,
    pub data_locations: Vec<Location>,
    pub flow_control: FlowObservation,
    pub fc_observation: String,
    pub protocol_compliance: String,
    pub capture_gaps_before: u64,
}
impl Event {
    fn issue(route: &Route, direction: Direction, record: &FrameRecord, reason: &str) -> Self {
        Self {
            kind: "protocol_issue".into(),
            route: Some(route.name.clone()),
            direction: Some(direction),
            status: "protocol_violation".into(),
            reason: Some(reason.into()),
            first_timestamp_ns: Some(record.frame.timestamp_ns()),
            last_timestamp_ns: Some(record.frame.timestamp_ns()),
            first_location: record.location.clone(),
            last_location: record.location.clone(),
            declared_length: None,
            observed_length: 0,
            data_hex: String::new(),
            data_locations: vec![],
            flow_control: FlowObservation::default(),
            fc_observation: "unknown".into(),
            protocol_compliance: "unknown".into(),
            capture_gaps_before: 0,
        }
    }
}
fn hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(data.len() * 2);
    for byte in data {
        let _ = write!(text, "{byte:02X}");
    }
    text
}
pub fn stmin_ns(byte: u8) -> Option<i64> {
    match byte {
        0..=0x7f => Some(i64::from(byte) * 1_000_000),
        0xf1..=0xf9 => Some(i64::from(byte - 0xf0) * 100_000),
        _ => None,
    }
}
struct Session {
    declared: usize,
    data: Vec<u8>,
    next_sequence: u8,
    first_ns: i64,
    last_ns: i64,
    first: Location,
    last: Location,
    locations: Vec<Location>,
    flow: FlowObservation,
    last_cf: Option<i64>,
    block_count: u64,
    flow_status: Option<u8>,
    gaps: u64,
}
impl Session {
    fn event(
        self,
        route: &Route,
        direction: Direction,
        status: &str,
        reason: Option<&str>,
        multi: bool,
    ) -> Event {
        Event {
            kind: "payload".into(),
            route: Some(route.name.clone()),
            direction: Some(direction),
            status: status.into(),
            reason: reason.map(str::to_owned),
            first_timestamp_ns: Some(self.first_ns),
            last_timestamp_ns: Some(self.last_ns),
            first_location: self.first,
            last_location: self.last,
            declared_length: Some(self.declared),
            observed_length: self.data.len(),
            data_hex: hex(&self.data),
            data_locations: self.locations,
            fc_observation: if !multi {
                "not_required"
            } else if self.flow.count == 0 {
                "missing"
            } else {
                "observed"
            }
            .into(),
            flow_control: self.flow,
            protocol_compliance: "unknown".into(),
            capture_gaps_before: self.gaps,
        }
    }
}

pub struct Reassembler {
    config: Config,
    sessions: Vec<[Option<Session>; 2]>,
    reserved: usize,
    pub matched_frames: u64,
    pub capture_gaps: u64,
    last_time: Option<i64>,
}
pub struct ResponseStart {
    pub route: String,
    pub first_timestamp_ns: i64,
    pub first_ordinal: u64,
}
impl Reassembler {
    pub fn receiving_responses(&self) -> Vec<ResponseStart> {
        self.sessions
            .iter()
            .enumerate()
            .filter_map(|(index, sessions)| {
                sessions[Direction::Response.index()]
                    .as_ref()
                    .map(|session| ResponseStart {
                        route: self.config.routes[index].name.clone(),
                        first_timestamp_ns: session.first_ns,
                        first_ordinal: session.first.ordinal,
                    })
            })
            .collect()
    }
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        let sessions = (0..config.routes.len()).map(|_| [None, None]).collect();
        Ok(Self {
            config,
            sessions,
            reserved: 0,
            matched_frames: 0,
            capture_gaps: 0,
            last_time: None,
        })
    }
    pub fn active_sessions(&self) -> usize {
        self.sessions
            .iter()
            .flatten()
            .filter(|s| s.is_some())
            .count()
    }
    pub fn reserved_payload_bytes(&self) -> usize {
        self.reserved
    }
    /// Validate the entire source clock, including unrelated frames and dated issues.
    pub fn advance(&mut self, timestamp: i64, location: &Location) -> Result<Vec<Event>> {
        ensure!(self.last_time.is_none_or(|last| timestamp >= last), "ISO-TP timestamp regression at {location:?}: {} -> {timestamp}; ordered single-clock input required", self.last_time.unwrap_or(timestamp));
        self.last_time = Some(timestamp);
        let mut events = vec![];
        for route in 0..self.sessions.len() {
            for direction in [Direction::Request, Direction::Response] {
                if self.sessions[route][direction.index()]
                    .as_ref()
                    .is_some_and(|s| timestamp - s.last_ns > self.config.routes[route].timeout_ns)
                {
                    events.push(
                        self.finish(
                            route,
                            direction,
                            "incomplete",
                            Some("timeout_observed_in_log"),
                        )
                        .unwrap(),
                    );
                }
            }
        }
        Ok(events)
    }
    fn finish(
        &mut self,
        route: usize,
        direction: Direction,
        status: &str,
        reason: Option<&str>,
    ) -> Option<Event> {
        let session = self.sessions[route][direction.index()].take()?;
        self.reserved -= session.declared;
        Some(session.event(&self.config.routes[route], direction, status, reason, true))
    }
    pub fn eof(&mut self) -> Vec<Event> {
        self.flush(None, "end_of_file")
    }
    fn flush(&mut self, channel: Option<u16>, reason: &str) -> Vec<Event> {
        let mut events = vec![];
        for route in 0..self.sessions.len() {
            if channel.is_none_or(|c| c == self.config.routes[route].channel) {
                for direction in [Direction::Request, Direction::Response] {
                    if let Some(event) = self.finish(route, direction, "incomplete", Some(reason)) {
                        events.push(event);
                    }
                }
            }
        }
        events
    }
    pub fn gap(&mut self, channel: Option<u16>) -> Vec<Event> {
        self.capture_gaps += 1;
        self.flush(channel, "capture_gap")
    }
    fn endpoint(&self, record: &FrameRecord) -> Option<(usize, Direction)> {
        let frame = &record.frame;
        for (i, route) in self.config.routes.iter().enumerate() {
            if frame.channel() != route.channel {
                continue;
            }
            for (direction, endpoint) in [
                (Direction::Request, &route.request),
                (Direction::Response, &route.response),
            ] {
                if frame.id() == endpoint.id
                    && frame.extended() == endpoint.extended
                    && endpoint
                        .address
                        .is_none_or(|address| frame.data().first() == Some(&address))
                {
                    return Some((i, direction));
                }
            }
        }
        None
    }
    fn invalid(
        &mut self,
        route: usize,
        direction: Direction,
        record: &FrameRecord,
        reason: &str,
        target: Direction,
        events: &mut Vec<Event>,
    ) {
        if let Some(event) = self.finish(route, target, "incomplete", Some(reason)) {
            events.push(event);
        }
        let mut issue = Event::issue(&self.config.routes[route], direction, record, reason);
        issue.capture_gaps_before = self.capture_gaps;
        events.push(issue);
    }
    fn padding(&self, route: usize, bytes: &[u8]) -> bool {
        self.config.routes[route]
            .padding_byte
            .is_none_or(|byte| bytes.iter().all(|b| *b == byte))
    }
    pub fn feed(&mut self, record: &FrameRecord) -> Result<Vec<Event>> {
        let frame = &record.frame;
        let mut events = self.advance(frame.timestamp_ns(), &record.location)?;
        let Some((route, direction)) = self.endpoint(record) else {
            return Ok(events);
        };
        self.matched_frames += 1;
        let prefix = usize::from(self.config.routes[route].addressing != Addressing::Normal);
        let bytes = &frame.data()[prefix.min(frame.data().len())..];
        let kind = bytes.first().map(|b| b >> 4);
        let target = if kind == Some(3) {
            direction.opposite()
        } else {
            direction
        };
        if frame.fd() || frame.remote() || bytes.is_empty() {
            self.invalid(
                route,
                direction,
                record,
                "unsupported_frame_profile_or_empty_payload",
                target,
                &mut events,
            );
            return Ok(events);
        }
        match kind.unwrap() {
            0 => {
                let length = usize::from(bytes[0] & 15);
                if length == 0
                    || length > bytes.len() - 1
                    || length > 7 - prefix
                    || !self.padding(route, &bytes[1 + length.min(bytes.len() - 1)..])
                {
                    self.invalid(
                        route,
                        direction,
                        record,
                        "invalid_single_frame_or_padding",
                        target,
                        &mut events,
                    );
                } else if length > self.config.max_payload_bytes {
                    self.invalid(
                        route,
                        direction,
                        record,
                        "payload_limit",
                        target,
                        &mut events,
                    );
                } else {
                    if let Some(event) = self.finish(
                        route,
                        direction,
                        "aborted",
                        Some("replaced_by_single_frame"),
                    ) {
                        events.push(event);
                    }
                    let session = Session {
                        declared: length,
                        data: bytes[1..1 + length].to_vec(),
                        next_sequence: 1,
                        first_ns: frame.timestamp_ns(),
                        last_ns: frame.timestamp_ns(),
                        first: record.location.clone(),
                        last: record.location.clone(),
                        locations: vec![record.location.clone()],
                        flow: FlowObservation::default(),
                        last_cf: None,
                        block_count: 0,
                        flow_status: None,
                        gaps: self.capture_gaps,
                    };
                    events.push(session.event(
                        &self.config.routes[route],
                        direction,
                        "complete",
                        None,
                        false,
                    ));
                }
            }
            1 => {
                if let Some(event) =
                    self.finish(route, direction, "aborted", Some("replaced_by_first_frame"))
                {
                    events.push(event);
                }
                let length = if bytes.len() >= 2 {
                    (usize::from(bytes[0] & 15) << 8) | usize::from(bytes[1])
                } else {
                    0
                };
                let reason = if frame.data().len() != 8 || length <= 7 - prefix {
                    Some("invalid_first_frame_or_unsupported_length_escape")
                } else if length > self.config.max_payload_bytes {
                    Some("payload_limit")
                } else if self.active_sessions() >= self.config.max_sessions
                    || self.reserved + length > self.config.max_total_payload_bytes
                {
                    Some("session_or_total_payload_limit")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.invalid(route, direction, record, reason, target, &mut events);
                } else {
                    // Reserve declared bytes only after all per-session/global limits pass.
                    let mut data = Vec::with_capacity(length);
                    data.extend_from_slice(&bytes[2..]);
                    self.reserved += length;
                    self.sessions[route][direction.index()] = Some(Session {
                        declared: length,
                        data,
                        next_sequence: 1,
                        first_ns: frame.timestamp_ns(),
                        last_ns: frame.timestamp_ns(),
                        first: record.location.clone(),
                        last: record.location.clone(),
                        locations: vec![record.location.clone()],
                        flow: FlowObservation::default(),
                        last_cf: None,
                        block_count: 0,
                        flow_status: None,
                        gaps: self.capture_gaps,
                    });
                }
            }
            2 => {
                if let Some(session) = &self.sessions[route][direction.index()] {
                    let remaining = session.declared - session.data.len();
                    let available = bytes.len() - 1;
                    let reason = if bytes[0] & 15 != session.next_sequence {
                        Some("sequence_mismatch_possible_capture_loss")
                    } else if available == 0 || (available < remaining && frame.data().len() != 8) {
                        Some("truncated_consecutive_frame")
                    } else if available >= remaining
                        && !self.padding(route, &bytes[1 + remaining..])
                    {
                        Some("invalid_final_padding")
                    } else {
                        None
                    };
                    if let Some(reason) = reason {
                        self.invalid(route, direction, record, reason, target, &mut events);
                    } else {
                        let session = self.sessions[route][direction.index()].as_mut().unwrap();
                        let now = frame.timestamp_ns();
                        if let Some(last) = session.last_cf {
                            let delta = now - last;
                            session.flow.minimum_cf_delta_ns = Some(
                                session
                                    .flow
                                    .minimum_cf_delta_ns
                                    .map_or(delta, |old| old.min(delta)),
                            );
                            if session
                                .flow
                                .last_stmin_ns
                                .is_some_and(|stmin| delta < stmin)
                            {
                                session.flow.shorter_than_stmin += 1;
                            }
                        }
                        session.block_count += 1;
                        if session.flow_status == Some(1) {
                            session.flow.cf_after_wait += 1;
                        }
                        if session.flow_status == Some(0)
                            && session
                                .flow
                                .last_block_size
                                .is_some_and(|bs| bs != 0 && session.block_count > u64::from(bs))
                        {
                            session.flow.block_size_exceeded += 1;
                        }
                        session.last_cf = Some(now);
                        session.last_ns = now;
                        session.last = record.location.clone();
                        session.locations.push(record.location.clone());
                        session
                            .data
                            .extend_from_slice(&bytes[1..1 + remaining.min(available)]);
                        session.next_sequence = (session.next_sequence + 1) & 15;
                        if session.data.len() == session.declared {
                            events.push(self.finish(route, direction, "complete", None).unwrap());
                        }
                    }
                } else {
                    self.invalid(
                        route,
                        direction,
                        record,
                        "orphan_consecutive_frame",
                        target,
                        &mut events,
                    );
                }
            }
            3 => {
                let status = bytes[0] & 15;
                if bytes.len() < 3
                    || status > 2
                    || !self.padding(route, &bytes[3.min(bytes.len())..])
                {
                    self.invalid(
                        route,
                        direction,
                        record,
                        "invalid_flow_control_or_padding",
                        target,
                        &mut events,
                    );
                } else {
                    let mut event = Event::issue(
                        &self.config.routes[route],
                        direction,
                        record,
                        "orphan_flow_control",
                    );
                    event.kind = "flow_control".into();
                    event.status = "observed".into();
                    event.capture_gaps_before = self.capture_gaps;
                    let observation = &mut event.flow_control;
                    observation.count = 1;
                    observation.cts = u64::from(status == 0);
                    observation.wait = u64::from(status == 1);
                    observation.overflow = u64::from(status == 2);
                    observation.last_block_size = Some(bytes[1]);
                    observation.last_stmin_byte = Some(bytes[2]);
                    observation.last_stmin_ns = stmin_ns(bytes[2]);
                    observation.reserved_stmin = u64::from(observation.last_stmin_ns.is_none());
                    observation.locations.push(record.location.clone());
                    event.fc_observation = "observed".into();
                    if let Some(session) = self.sessions[route][target.index()].as_mut() {
                        event.reason = None;
                        session.last_ns = frame.timestamp_ns();
                        session.last = record.location.clone();
                        session.flow.count += 1;
                        session.flow.cts += observation.cts;
                        session.flow.wait += observation.wait;
                        session.flow.overflow += observation.overflow;
                        // STmin/BS are meaningful only on CTS, preserve WAIT bytes in the FC event.
                        if status == 0 {
                            session.flow.reserved_stmin += observation.reserved_stmin;
                            session.flow.last_block_size = observation.last_block_size;
                            session.flow.last_stmin_byte = observation.last_stmin_byte;
                            session.flow.last_stmin_ns = observation.last_stmin_ns;
                            session.block_count = 0;
                        }
                        session.flow_status = Some(status);
                        if session.flow.locations.len() < 16 {
                            session.flow.locations.push(record.location.clone());
                        } else {
                            session.flow.omitted_locations += 1;
                        }
                    }
                    events.push(event);
                    if status == 2 {
                        if let Some(event) = self.finish(
                            route,
                            target,
                            "aborted",
                            Some("observed_flow_control_overflow"),
                        ) {
                            events.push(event);
                        }
                    }
                }
            }
            _ => self.invalid(
                route,
                direction,
                record,
                "invalid_pci_type",
                target,
                &mut events,
            ),
        }
        Ok(events)
    }
}
