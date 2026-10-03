//! Optional pinned CDD facade. No XML lexer or diagnostic field codec is implemented here.
#[cfg(feature = "cdd")]
use crate::uds::Status;
use crate::{
    playback::Cancellation,
    uds::{Config, Observation},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
pub const ENGINE_REVISION: &str = "ecd4a6a42792636a8386439653950e8693818d02";
pub fn inspect(
    path: &std::path::Path,
    allow_experimental: bool,
    cancel: &Cancellation,
) -> Result<Value> {
    #[cfg(not(feature = "cdd"))]
    {
        let _ = (path, allow_experimental, cancel);
        anyhow::bail!("CDD support is not built; build with scripts/build.ps1 -Cdd (Rust 1.98.1+)")
    }
    #[cfg(feature = "cdd")]
    {
        use std::{fs::File, io::Read};
        ensure!(
            allow_experimental,
            "CDD profile is experimental; specify --allow-experimental"
        );
        cancel.check()?;
        let mut bytes = vec![];
        File::open(path)?.take(8_388_609).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 8_388_608, "CDD file exceeds 8 MiB");
        let engine = cdd_api::CddEngine::open(bytes, &cdd_api::OpenOptions { allow_experimental })?;
        cancel.check()?;
        let model = engine.model();
        let fingerprint = engine.fingerprint();
        let hash: String = fingerprint
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let profile_hash: String = fingerprint
            .profile_fingerprint
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        ensure!(
            crate::index::hash_file(path, cancel)? == hash,
            "CDD changed during inspection"
        );
        Ok(
            serde_json::json!({"schema_version":1,"engine_revision":ENGINE_REVISION,"path":path,"sha256":hash,"profile_maturity":"experimental","profile_fingerprint":profile_hash,"parser_semver":fingerprint.parser_semver,"protocol":format!("{:?}",model.protocol),"ecus":model.ecus.iter().take(128).map(|e|serde_json::json!({"key":e.key,"qualifier":e.qualifier,"variants":e.variants.iter().take(128).map(|i|serde_json::json!({"key":model.variants[*i].key,"qualifier":model.variants[*i].qualifier})).collect::<Vec<_>>(),"omitted_variants":e.variants.len().saturating_sub(128)})).collect::<Vec<_>>(),"omitted_ecus":model.ecus.len().saturating_sub(128),"service_count":model.services.len(),"services":model.services.iter().take(128).map(|s|serde_json::json!({"key":s.key,"sid":s.sid,"protocol":format!("{:?}",s.protocol)})).collect::<Vec<_>>(),"omitted_services":model.services.len().saturating_sub(128),"did_count":engine.dids().len(),"dids":engine.dids().iter().take(128).map(|d|serde_json::json!({"identifier":d.identifier,"qualifier":d.qualifier,"variant":model.variants[d.variant].key})).collect::<Vec<_>>(),"omitted_dids":engine.dids().len().saturating_sub(128),"load_issue_count":engine.load_issues().len(),"load_error_count":engine.load_issues().iter().filter(|i|i.severity.label()=="error").count(),"load_issue_examples":engine.load_issues().iter().take(100).map(|i|serde_json::json!({"severity":i.severity.label(),"code":i.code,"message":i.message})).collect::<Vec<_>>(),"omitted_load_issue_examples":engine.load_issues().len().saturating_sub(100)}),
        )
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub path: PathBuf,
    pub ecu: String,
    pub variant: String,
    #[serde(default)]
    pub allow_experimental: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub route: String,
    pub path: PathBuf,
    pub sha256: String,
    pub engine_revision: &'static str,
    pub ecu: String,
    pub variant: String,
    pub profile_maturity: &'static str,
    pub profile_fingerprint: String,
    pub parser_semver: String,
    pub load_issue_count: usize,
    pub load_error_count: usize,
    pub load_issue_examples: Vec<Value>,
    pub omitted_load_issue_examples: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct Decoded {
    pub status: String,
    pub document_sha256: String,
    pub engine_revision: &'static str,
    pub identification: Option<Value>,
    pub request_context: Option<Value>,
    pub request: Option<Value>,
    pub response: Option<Value>,
    pub error: Option<Value>,
}
#[cfg(feature = "cdd")]
struct Binding {
    route: String,
    engine: cdd_api::CddEngine,
    context: usize,
    sha256: String,
}
pub struct Decoder {
    pub summaries: Vec<Summary>,
    #[cfg(feature = "cdd")]
    bindings: Vec<Binding>,
}
/// An engine-created context kept with the bounded passive transaction.
pub struct NativeRequest {
    pub header: crate::uds::RequestHeader,
    pub decoded: Decoded,
    #[cfg(feature = "cdd")]
    context: cdd_api::RequestContext,
    #[cfg(feature = "cdd")]
    service: usize,
}
pub enum NativeResponse {
    NoMatch,
    Unverified(Decoded),
    Matched { nrc: Option<u8>, decoded: Decoded },
}
impl std::fmt::Debug for Decoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CddDecoder")
            .field("summaries", &self.summaries)
            .finish_non_exhaustive()
    }
}
impl Decoder {
    pub fn has_route(&self, route: &str) -> bool {
        self.summaries.iter().any(|s| s.route == route)
    }
    #[cfg(feature = "cdd")]
    fn binding(&self, route: &str) -> Result<&Binding> {
        self.bindings
            .iter()
            .find(|b| b.route == route)
            .ok_or_else(|| anyhow::anyhow!("KWP2000 route {route:?} has no CDD engine"))
    }
    #[cfg(feature = "cdd")]
    fn identification(binding: &Binding, identified: &cdd_api::IdentifyResult) -> Decoded {
        Decoded {
            status: match identified {
                cdd_api::IdentifyResult::NoMatch => "no_match",
                cdd_api::IdentifyResult::Unique(_) => "identified",
                cdd_api::IdentifyResult::Ambiguous(_) => "ambiguous",
                cdd_api::IdentifyResult::Unverified(_) => "unverified",
            }
            .into(),
            document_sha256: binding.sha256.clone(),
            engine_revision: ENGINE_REVISION,
            identification: Some(
                serde_json::json!({"status":identified.label(),"candidates":identified.candidates().iter().map(|c|serde_json::json!({"service":binding.engine.model().services[c.service].key,"message":c.kind.label(),"verification":format!("{:?}",c.verification),"matched_prefix_bytes":c.matched_prefix_bytes})).collect::<Vec<_>>() }),
            ),
            request_context: None,
            request: None,
            response: None,
            error: None,
        }
    }
    /// No SID tables or byte-offset rules: the selected CDD decides the request identity.
    pub fn kwp_request(
        &self,
        route: &str,
        payload: &[u8],
    ) -> Result<(Option<NativeRequest>, Decoded)> {
        #[cfg(not(feature = "cdd"))]
        {
            let _ = (route, payload);
            anyhow::bail!("KWP2000 requires a build with CDD support")
        }
        #[cfg(feature = "cdd")]
        {
            use cdd_api::{Direction, IdentifyResult, MessageKind};
            let binding = self.binding(route)?;
            let engine = &binding.engine;
            let identified = engine.identify(binding.context, Direction::Request, payload)?;
            let mut decoded = Self::identification(binding, &identified);
            let IdentifyResult::Unique(candidate) = identified else {
                return Ok((None, decoded));
            };
            let message = engine.decode_message(
                candidate.service,
                MessageKind::Request,
                payload,
                &cdd_api::DecodeOptions::default(),
            )?;
            let context = engine.request_context(candidate.service, payload)?;
            ensure!(
                context.document == *engine.fingerprint(),
                "CDD request context fingerprint mismatch"
            );
            decoded.request_context = Some(context.to_json());
            decoded.request = Some(cdd_api::json::decoded_message(engine.model(), &message));
            let service = &engine.model().services[candidate.service];
            let header = crate::uds::RequestHeader {
                service_id: service
                    .sid
                    .ok_or_else(|| anyhow::anyhow!("CDD request service has no SID"))?,
                service_key: Some(service.key.clone()),
                ..Default::default()
            };
            Ok((
                Some(NativeRequest {
                    header,
                    decoded: decoded.clone(),
                    context,
                    service: candidate.service,
                }),
                decoded,
            ))
        }
    }
    /// Candidate filtering and full response verification both belong to cdd_engine.
    pub fn kwp_response(
        &self,
        route: &str,
        request: &NativeRequest,
        payload: &[u8],
    ) -> Result<NativeResponse> {
        #[cfg(not(feature = "cdd"))]
        {
            let _ = (route, request, payload);
            anyhow::bail!("KWP2000 requires a build with CDD support")
        }
        #[cfg(feature = "cdd")]
        {
            use cdd_api::{DecodedResponse, Direction, Verification};
            let binding = self.binding(route)?;
            let engine = &binding.engine;
            ensure!(
                request.context.document == *engine.fingerprint(),
                "CDD request context fingerprint mismatch"
            );
            let identified = engine.identify(binding.context, Direction::Response, payload)?;
            let Some(candidate) = identified
                .candidates()
                .iter()
                .find(|c| c.service == request.service)
            else {
                return Ok(NativeResponse::NoMatch);
            };
            let mut decoded = request.decoded.clone();
            if candidate.verification != Verification::FullyDecoded {
                decoded.status = "unverified".into();
                decoded.error = Some(
                    serde_json::json!({"code":"CANLOG-CDD-002","message":"CDD response layout could only verify the prefix","verification":format!("{:?}",candidate.verification)}),
                );
                return Ok(NativeResponse::Unverified(decoded));
            }
            match engine.decode_response(
                &request.context,
                payload,
                &cdd_api::DecodeOptions::default(),
            ) {
                Ok(message) => {
                    let nrc = match &message {
                        DecodedResponse::Negative(n) => Some(n.nrc),
                        DecodedResponse::Positive(_) => None,
                    };
                    let response = cdd_api::json::decoded_response(engine.model(), &message);
                    let diagnostics = response
                        .get("diagnostics")
                        .and_then(Value::as_array)
                        .is_some_and(|v| !v.is_empty())
                        || decoded
                            .request
                            .as_ref()
                            .and_then(|r| r.get("diagnostics"))
                            .and_then(Value::as_array)
                            .is_some_and(|v| !v.is_empty());
                    decoded.status = if diagnostics {
                        "decoded_with_diagnostics"
                    } else {
                        "decoded"
                    }
                    .into();
                    decoded.response = Some(response);
                    Ok(NativeResponse::Matched { nrc, decoded })
                }
                Err(error) => {
                    decoded.status = "decode_error".into();
                    decoded.error = Some(error.to_json());
                    Ok(NativeResponse::Unverified(decoded))
                }
            }
        }
    }
    pub fn kwp_response_identification(&self, route: &str, payload: &[u8]) -> Result<Decoded> {
        #[cfg(not(feature = "cdd"))]
        {
            let _ = (route, payload);
            anyhow::bail!("KWP2000 requires a build with CDD support")
        }
        #[cfg(feature = "cdd")]
        {
            let binding = self.binding(route)?;
            let identified =
                binding
                    .engine
                    .identify(binding.context, cdd_api::Direction::Response, payload)?;
            Ok(Self::identification(binding, &identified))
        }
    }
    pub fn load(config: &Config, cancel: &Cancellation) -> Result<Self> {
        let entries: Vec<_> = config
            .routes
            .iter()
            .filter_map(|r| r.cdd.as_ref().map(|a| (r, a)))
            .collect();
        ensure!(
            entries.len() <= 8,
            "at most 8 CDD assignments are supported"
        );
        cancel.check()?;
        #[cfg(not(feature = "cdd"))]
        {
            ensure!(entries.is_empty(),"CDD support is not built; build with scripts/build.ps1 -Cdd (Rust 1.98.1+) or Cargo --features cdd");
            Ok(Self { summaries: vec![] })
        }
        #[cfg(feature = "cdd")]
        {
            use cdd_api::{CddEngine, OpenOptions};
            use std::{fs::File, io::Read};
            let mut result = Self {
                summaries: vec![],
                bindings: vec![],
            };
            let mut total = 0usize;
            for (route, assignment) in entries {
                cancel.check()?;
                ensure!(
                    !assignment.ecu.is_empty()
                        && assignment.ecu.len() <= 128
                        && !assignment.variant.is_empty()
                        && assignment.variant.len() <= 128,
                    "CDD ECU/variant qualifiers must be 1..128 bytes"
                );
                ensure!(assignment.allow_experimental,"CDD engine profile is experimental; set allow_experimental=true explicitly in the route CDD assignment");
                let mut bytes = vec![];
                File::open(&assignment.path)?
                    .take(8_388_609)
                    .read_to_end(&mut bytes)?;
                ensure!(bytes.len() <= 8_388_608, "CDD file exceeds 8 MiB");
                total += bytes.len();
                ensure!(total <= 33_554_432, "CDD assignment sources exceed 32 MiB");
                cancel.check()?;
                let engine = CddEngine::open(
                    bytes,
                    &OpenOptions {
                        allow_experimental: assignment.allow_experimental,
                    },
                )?;
                ensure!(
                    engine.model().protocol.label() == route.protocol.cdd_label(),
                    "CDD protocol {:?} does not match the explicit route policy {:?}",
                    engine.model().protocol,
                    route.protocol
                );
                let ecu = engine.select_ecu(&assignment.ecu)?;
                let variant = engine.select_variant(ecu, &assignment.variant)?;
                let context = engine.context(ecu, variant)?;
                let fingerprint = engine.fingerprint();
                fn hex(bytes: &[u8]) -> String {
                    bytes.iter().map(|b| format!("{b:02x}")).collect()
                }
                let hash = hex(&fingerprint.sha256);
                let issues = engine.load_issues();
                result.summaries.push(Summary {route:route.route.clone(),path:assignment.path.clone(),sha256:hash.clone(),engine_revision:ENGINE_REVISION,ecu:assignment.ecu.clone(),variant:assignment.variant.clone(),profile_maturity:"experimental",profile_fingerprint:hex(&fingerprint.profile_fingerprint),parser_semver:fingerprint.parser_semver.clone(),load_issue_count:issues.len(),load_error_count:issues.iter().filter(|i|i.severity.label()=="error").count(),load_issue_examples:issues.iter().take(100).map(|i|serde_json::json!({"severity":i.severity.label(),"code":i.code,"message":i.message})).collect(),omitted_load_issue_examples:issues.len().saturating_sub(100)});
                result.bindings.push(Binding {
                    route: route.route.clone(),
                    engine,
                    context,
                    sha256: hash,
                });
            }
            Ok(result)
        }
    }
    pub fn decorate(&self, observation: &mut Observation) {
        if observation.cdd.is_some() {
            return;
        }
        #[cfg(not(feature = "cdd"))]
        let _ = observation;
        #[cfg(feature = "cdd")]
        {
            let Some(binding) = self.bindings.iter().find(|b| b.route == observation.route) else {
                return;
            };
            let mut decoded = Decoded {
                status: "not_decoded".into(),
                document_sha256: binding.sha256.clone(),
                engine_revision: ENGINE_REVISION,
                identification: None,
                request_context: None,
                request: None,
                response: None,
                error: None,
            };
            if matches!(
                observation.status,
                Status::Positive | Status::Negative | Status::Pending
            ) {
                let attempt = (|| -> Result<()> {
                    use cdd_api::{
                        DecodeOptions, Direction, IdentifyResult, MessageKind, TrailingBytes,
                    };
                    let request = observation
                        .request
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("request context is unavailable"))?;
                    let response = observation
                        .response
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("response is unavailable"))?;
                    fn bytes(text: &str) -> Result<Vec<u8>> {
                        ensure!(
                            text.is_ascii() && text.len() <= 8190 && text.len().is_multiple_of(2),
                            "invalid UDS payload hex"
                        );
                        (0..text.len())
                            .step_by(2)
                            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(Into::into))
                            .collect()
                    }
                    let request = bytes(&request.data_hex)?;
                    let response = bytes(&response.data_hex)?;
                    let engine = &binding.engine;
                    let identified =
                        engine.identify(binding.context, Direction::Request, &request)?;
                    decoded.status = match &identified {
                        IdentifyResult::NoMatch => "no_match",
                        IdentifyResult::Unique(_) => "identified",
                        IdentifyResult::Ambiguous(_) => "ambiguous",
                        IdentifyResult::Unverified(_) => "unverified",
                    }
                    .into();
                    decoded.identification = Some(
                        serde_json::json!({"status":identified.label(),"candidates":identified.candidates().iter().map(|c|serde_json::json!({"service":engine.model().services[c.service].key,"message":c.kind.label(),"verification":format!("{:?}",c.verification),"matched_prefix_bytes":c.matched_prefix_bytes})).collect::<Vec<_>>() }),
                    );
                    let IdentifyResult::Unique(candidate) = identified else {
                        return Ok(());
                    };
                    ensure!(
                        candidate.kind == MessageKind::Request,
                        "identified CDD message is not a request"
                    );
                    let options = DecodeOptions {
                        trailing: TrailingBytes::Error,
                        reserved_bits: false,
                    };
                    let request_message=engine.decode_message(candidate.service,MessageKind::Request,&request,&options).inspect_err(|error|{
                        decoded.error=Some(serde_json::json!({"code":error.code(),"message":error.to_string()}));
                    })?;
                    let context = engine.request_context(candidate.service, &request)?;
                    ensure!(
                        context.document == *engine.fingerprint(),
                        "CDD request context fingerprint mismatch"
                    );
                    decoded.request_context = Some(context.to_json());
                    decoded.request = Some(cdd_api::json::decoded_message(
                        engine.model(),
                        &request_message,
                    ));
                    let response_message=engine.decode_response(&context,&response,&options).inspect_err(|error|{
                        decoded.error=Some(serde_json::json!({"code":error.code(),"message":error.to_string()}));
                    })?;
                    let response_json =
                        cdd_api::json::decoded_response(engine.model(), &response_message);
                    let diagnostics = response_json
                        .get("diagnostics")
                        .and_then(Value::as_array)
                        .is_some_and(|v| !v.is_empty())
                        || !request_message.diagnostics.is_empty();
                    decoded.response = Some(response_json);
                    decoded.status = if diagnostics {
                        "decoded_with_diagnostics"
                    } else {
                        "decoded"
                    }
                    .into();
                    observation.data_semantics = "cdd_fields_available";
                    Ok(())
                })();
                if let Err(error) = attempt {
                    decoded.status = "decode_error".into();
                    if decoded.error.is_none() {
                        decoded.error = Some(
                            serde_json::json!({"code":"CANLOG-CDD-001","message":error.to_string()}),
                        );
                    }
                }
            }
            observation.cdd = Some(decoded);
        }
    }
}
