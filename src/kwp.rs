//! Narrow KWP2000-over-ISO-TP profile for the installed Vector CANSystem demos.
//! This does not parse K-line framing or manufacturer-specific data records.
use crate::uds::{RequestHeader, ResponseHeader};

fn supported(sid: u8) -> bool {
    matches!(
        sid,
        0x10 | 0x11 | 0x14 | 0x18 | 0x1a | 0x20 | 0x21 | 0x3b | 0x3e
    )
}
pub(crate) fn request_header(bytes: &[u8]) -> Result<RequestHeader, &'static str> {
    let sid = *bytes.first().ok_or("malformed_empty_request")?;
    if !supported(sid) {
        return Err("unsupported_kwp_service_profile");
    }
    let mut header = RequestHeader {
        service_id: sid,
        ..Default::default()
    };
    let valid = match sid {
        0x10 if bytes.len() == 2 => {
            header.diagnostic_mode = Some(bytes[1]);
            true
        }
        0x11 if bytes.len() == 2 => {
            header.reset_mode = Some(bytes[1]);
            true
        }
        0x14 if bytes.len() == 3 => {
            header.dtc_group = Some(u16::from_be_bytes([bytes[1], bytes[2]]));
            true
        }
        0x18 if bytes.len() == 4 => {
            if !matches!(bytes[1], 2 | 3) {
                return Err("unsupported_kwp_dtc_report_mode");
            }
            header.subfunction = Some(bytes[1]);
            header.dtc_group = Some(u16::from_be_bytes([bytes[2], bytes[3]]));
            true
        }
        0x1a | 0x21 if bytes.len() == 2 => {
            header.local_identifier = Some(bytes[1]);
            true
        }
        0x3b if bytes.len() >= 3 => {
            header.local_identifier = Some(bytes[1]);
            true
        }
        0x20 => bytes.len() == 1,
        0x3e if bytes.len() == 2 => {
            if !matches!(bytes[1], 1 | 2) {
                return Err("unsupported_kwp_tester_present_mode");
            }
            header.subfunction = Some(bytes[1]);
            header.suppress_positive_response = bytes[1] == 2;
            true
        }
        _ => false,
    };
    if valid {
        Ok(header)
    } else {
        Err("malformed_kwp_request_header")
    }
}
pub(crate) fn response_header(bytes: &[u8]) -> Result<ResponseHeader, &'static str> {
    let first = *bytes.first().ok_or("malformed_empty_response")?;
    if first == 0x7f {
        if bytes.len() != 3 || bytes[2] == 0 {
            return Err("malformed_negative_response");
        }
        if !supported(bytes[1]) {
            return Err("unsupported_kwp_negative_service_profile");
        }
        return Ok(ResponseHeader {
            sid: bytes[1],
            nrc: Some(bytes[2]),
        });
    }
    let sid = first
        .checked_sub(0x40)
        .ok_or("unsupported_kwp_response_profile")?;
    if !supported(sid) {
        return Err("unsupported_kwp_response_profile");
    }
    let valid = match sid {
        0x10 => bytes.len() == 2,
        // Legacy demo includes 51 00 00; retain the optional reset record opaque.
        0x11 => bytes.len() >= 2,
        0x14 => bytes.len() == 3,
        // Count + 2-byte DTC + 1-byte status per record; no report-mode echo.
        0x18 => bytes.len() >= 2 && bytes.len() == 2 + usize::from(bytes[1]) * 3,
        0x1a | 0x21 => bytes.len() >= 2,
        0x3b => bytes.len() == 2,
        0x20 | 0x3e => bytes.len() == 1,
        _ => false,
    };
    if valid {
        Ok(ResponseHeader { sid, nrc: None })
    } else {
        Err("malformed_kwp_response_header")
    }
}
pub(crate) fn matches(header: &RequestHeader, response: &ResponseHeader, bytes: &[u8]) -> bool {
    if header.service_id != response.sid {
        return false;
    }
    if response.nrc.is_some() {
        return true;
    }
    match header.service_id {
        0x10 => header.diagnostic_mode == Some(bytes[1]),
        0x11 => header.reset_mode == Some(bytes[1]),
        0x14 => header.dtc_group == Some(u16::from_be_bytes([bytes[1], bytes[2]])),
        0x1a | 0x21 | 0x3b => header.local_identifier == Some(bytes[1]),
        0x18 | 0x20 => true,
        // No positive response definition exists for mode 02 in this profile.
        0x3e => header.subfunction == Some(1),
        _ => false,
    }
}
