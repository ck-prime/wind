//! Transport-independent service protocol.
//!
//! Messages are single lines. The supervisor and a service executable share
//! this encoding. It does not open a socket, grant a permission, or carry
//! secrets.

use crate::{Capability, HealthStatus, ServiceId, Version};

/// Wire protocol implemented by this build. An incompatible peer is rejected.
pub const SERVICE_PROTOCOL_VERSION: u32 = 1;

/// Process lifetime. This is not service health, capability status, or permission state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    NotInstalled,
    Installed,
    Starting,
    Running,
    Stopping,
    Stopped,
    Crashed,
    Updating,
    Rollback,
}

impl ProcessState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "NotInstalled",
            Self::Installed => "Installed",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Crashed => "Crashed",
            Self::Updating => "Updating",
            Self::Rollback => "Rollback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceRequest {
    Hello,
    Health,
    CurrentApplication,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyMessage {
    pub service_id: ServiceId,
    pub version: Version,
    pub protocol_version: u32,
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthMessage {
    pub status: HealthStatus,
    pub capability: crate::CapabilityStatus,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationMessage {
    pub present: bool,
    pub application_id: String,
    pub name: String,
    pub executable: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: String,
    pub expected: Option<u32>,
    pub received: Option<u32>,
}

impl ProtocolError {
    pub fn incompatible(expected: u32, received: u32) -> Self {
        Self {
            code: "incompatible".to_string(),
            expected: Some(expected),
            received: Some(received),
        }
    }

    pub fn explain(&self) -> String {
        match (self.expected, self.received) {
            (Some(expected), Some(received)) if self.code == "incompatible" => {
                format!("Incompatible service protocol expected: {expected} received: {received}")
            }
            _ => format!("service protocol error {}", self.code),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceResponse {
    Ready(ReadyMessage),
    Health(HealthMessage),
    Application(ApplicationMessage),
    Bye,
    Error(ProtocolError),
}

pub fn encode_request(request: &ServiceRequest) -> String {
    match request {
        ServiceRequest::Hello => "hello".to_string(),
        ServiceRequest::Health => "health".to_string(),
        ServiceRequest::CurrentApplication => "current_application".to_string(),
        ServiceRequest::Shutdown => "shutdown".to_string(),
    }
}

pub fn decode_request(line: &str) -> Result<ServiceRequest, ProtocolError> {
    match line.trim() {
        "hello" => Ok(ServiceRequest::Hello),
        "health" => Ok(ServiceRequest::Health),
        "current_application" => Ok(ServiceRequest::CurrentApplication),
        "shutdown" => Ok(ServiceRequest::Shutdown),
        _ => Err(ProtocolError {
            code: "unknown".to_string(),
            expected: None,
            received: None,
        }),
    }
}

pub fn encode_response(response: &ServiceResponse) -> String {
    match response {
        ServiceResponse::Ready(ready) => {
            let capabilities = ready
                .capabilities
                .iter()
                .map(|capability| capability.as_str())
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "ready id={} version={} protocol={} capabilities={}",
                ready.service_id.as_str(),
                ready.version,
                ready.protocol_version,
                encode_token(&capabilities)
            )
        }
        ServiceResponse::Health(health) => format!(
            "health status={} capability={} detail={}",
            health.status.as_str(),
            health.capability.as_str(),
            encode_token(&health.detail)
        ),
        ServiceResponse::Application(application) => format!(
            "application present={} id={} name={} executable={}",
            if application.present { "1" } else { "0" },
            encode_token(&application.application_id),
            encode_token(&application.name),
            encode_token(&application.executable)
        ),
        ServiceResponse::Bye => "bye".to_string(),
        ServiceResponse::Error(error) => match (error.expected, error.received) {
            (Some(expected), Some(received)) => format!(
                "error code={} expected={} received={}",
                encode_token(&error.code),
                expected,
                received
            ),
            _ => format!("error code={}", encode_token(&error.code)),
        },
    }
}

pub fn decode_response(line: &str) -> Result<ServiceResponse, ProtocolError> {
    let line = line.trim();
    if line.len() > 8192 {
        return Err(ProtocolError {
            code: "too_long".to_string(),
            expected: None,
            received: None,
        });
    }
    let mut parts = line.split_whitespace();
    let tag = parts.next().unwrap_or("");
    let fields = parts.map(split_field).collect::<Vec<_>>();
    match tag {
        "ready" => Ok(ServiceResponse::Ready(ReadyMessage {
            service_id: ServiceId::parse(field(&fields, "id")?).ok_or(unknown())?,
            version: Version::parse(field(&fields, "version")?).map_err(|_| unknown())?,
            protocol_version: field(&fields, "protocol")?.parse().map_err(|_| unknown())?,
            capabilities: decode_capabilities(field(&fields, "capabilities")?)?,
        })),
        "health" => Ok(ServiceResponse::Health(HealthMessage {
            status: parse_health(field(&fields, "status")?)?,
            capability: parse_capability_status(field(&fields, "capability")?)?,
            detail: field(&fields, "detail")?.to_string(),
        })),
        "application" => Ok(ServiceResponse::Application(ApplicationMessage {
            present: field(&fields, "present")? == "1",
            application_id: field(&fields, "id")?.to_string(),
            name: field(&fields, "name")?.to_string(),
            executable: field(&fields, "executable")?.to_string(),
        })),
        "bye" => Ok(ServiceResponse::Bye),
        "error" => {
            let code = field(&fields, "code")?.to_string();
            let expected = optional_u32(&fields, "expected");
            let received = optional_u32(&fields, "received");
            Ok(ServiceResponse::Error(ProtocolError {
                code,
                expected,
                received,
            }))
        }
        _ => Err(unknown()),
    }
}

fn decode_capabilities(value: &str) -> Result<Vec<Capability>, ProtocolError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|item| Capability::parse(item).ok_or_else(unknown))
        .collect()
}

fn parse_health(value: &str) -> Result<HealthStatus, ProtocolError> {
    match value {
        "Healthy" => Ok(HealthStatus::Healthy),
        "Degraded" => Ok(HealthStatus::Degraded),
        "Unhealthy" => Ok(HealthStatus::Unhealthy),
        "Unknown" => Ok(HealthStatus::Unknown),
        "PermissionRequired" => Ok(HealthStatus::PermissionRequired),
        "Unsupported" => Ok(HealthStatus::Unsupported),
        _ => Err(unknown()),
    }
}

fn parse_capability_status(value: &str) -> Result<crate::CapabilityStatus, ProtocolError> {
    match value {
        "Available" => Ok(crate::CapabilityStatus::Available),
        "PermissionRequired" => Ok(crate::CapabilityStatus::PermissionRequired),
        "Unsupported" => Ok(crate::CapabilityStatus::Unsupported),
        "Unavailable" => Ok(crate::CapabilityStatus::Unavailable),
        "Unknown" => Ok(crate::CapabilityStatus::Unknown),
        _ => Err(unknown()),
    }
}

fn field<'a>(fields: &'a [(&'a str, String)], name: &str) -> Result<&'a str, ProtocolError> {
    fields
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.as_str())
        .ok_or_else(unknown)
}

fn optional_u32(fields: &[(&str, String)], name: &str) -> Option<u32> {
    field(fields, name).ok()?.parse().ok()
}

fn split_field(part: &str) -> (&str, String) {
    match part.split_once('=') {
        Some((key, value)) => (key, decode_token(value)),
        None => (part, String::new()),
    }
}

fn encode_token(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b',' => {
                encoded.push(*byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn decode_token(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).unwrap_or_default()
}

fn unknown() -> ProtocolError {
    ProtocolError {
        code: "malformed".to_string(),
        expected: None,
        received: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_request, decode_response, encode_request, encode_response, ApplicationMessage,
        HealthMessage, ProcessState, ProtocolError, ReadyMessage, ServiceRequest, ServiceResponse,
        SERVICE_PROTOCOL_VERSION,
    };
    use crate::{Capability, CapabilityStatus, HealthStatus, ServiceId, Version};

    #[test]
    fn protocol_round_trip_keeps_identity_and_version_apart() {
        let ready = ServiceResponse::Ready(ReadyMessage {
            service_id: ServiceId::AppTracker,
            version: Version::new(1, 2, 0),
            protocol_version: SERVICE_PROTOCOL_VERSION,
            capabilities: vec![Capability::ActiveApplication],
        });
        let decoded = decode_response(&encode_response(&ready)).unwrap();
        assert_eq!(decoded, ready);
        assert_eq!(
            decode_request(&encode_request(&ServiceRequest::CurrentApplication)).unwrap(),
            ServiceRequest::CurrentApplication
        );
    }

    #[test]
    fn incompatible_protocol_is_explicit() {
        let error = ProtocolError::incompatible(1, 2);
        let text = error.explain();
        assert!(text.contains("expected: 1"));
        assert!(text.contains("received: 2"));
        let encoded = encode_response(&ServiceResponse::Error(error.clone()));
        assert_eq!(
            decode_response(&encoded).unwrap(),
            ServiceResponse::Error(error)
        );
    }

    #[test]
    fn application_fields_survive_spaces() {
        let message = ServiceResponse::Application(ApplicationMessage {
            present: true,
            application_id: "app id".to_string(),
            name: "Visual Studio".to_string(),
            executable: "C:\\Program Files\\app.exe".to_string(),
        });
        assert_eq!(
            decode_response(&encode_response(&message)).unwrap(),
            message
        );
        let health = ServiceResponse::Health(HealthMessage {
            status: HealthStatus::Healthy,
            capability: CapabilityStatus::Available,
            detail: "running".to_string(),
        });
        assert_eq!(decode_response(&encode_response(&health)).unwrap(), health);
    }

    #[test]
    fn process_states_stay_separate_from_health() {
        assert_eq!(ProcessState::Crashed.as_str(), "Crashed");
        assert_ne!(
            ProcessState::Running.as_str(),
            HealthStatus::Healthy.as_str()
        );
    }
}
