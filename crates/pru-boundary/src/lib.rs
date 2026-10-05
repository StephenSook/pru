//! Compile a client's consent ledger into a REST-only OpenShell boundary.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Minimal non-sensitive view of one client's consent ledger.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConsentLedger {
    /// Synthetic or opaque client compartment identifier.
    pub client_id: String,
    /// Opaque identifiers of verified consent documents.
    pub consent_document_ids: Vec<String>,
}

/// Pru gateway service reached from the OpenShell sandbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayEndpoint {
    /// DNS hostname visible from the sandbox.
    pub host: String,
    /// REST port.
    pub port: u16,
}

/// Boundary compilation error.
#[derive(Debug, Error)]
pub enum BoundaryError {
    /// A ledger cannot identify a client compartment.
    #[error("client_id must not be empty")]
    EmptyClientId,
    /// Hostname would make the YAML ambiguous or invalid.
    #[error("gateway host contains unsupported characters: {0}")]
    InvalidGatewayHost(String),
}

/// Compile the boundary accepted by OpenShell 0.0.116 and prover 0.1.2.
///
/// Consent changes do not widen the network boundary. All actions still cross
/// the same gateway, where Biscuit and Cedar make the per-action decision.
pub fn compile_boundary(
    ledger: &ConsentLedger,
    gateway: &GatewayEndpoint,
) -> Result<String, BoundaryError> {
    if ledger.client_id.trim().is_empty() {
        return Err(BoundaryError::EmptyClientId);
    }
    if !valid_hostname(&gateway.host) {
        return Err(BoundaryError::InvalidGatewayHost(gateway.host.clone()));
    }

    Ok(format!(
        r#"version: 1
filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
    - /opt/hermes
    - /proc
    - /dev/urandom
    - /app
    - /run/nemoclaw/managed-startup-ca-bundle.pem
    - /run/nemoclaw/managed-startup-runtime.env
    - /run/nemoclaw/managed-gateway-expected-exit
    - /etc
    - /var/log
    - /var/lib/dpkg
  read_write:
    - /sandbox
    - /tmp
    - /dev/null
    - /dev/pts
    - /sandbox/.hermes
landlock:
  compatibility: best_effort
process:
  run_as_user: sandbox
  run_as_group: sandbox
network_policies:
  pru_gateway:
    name: pru_gateway
    endpoints:
      - host: {host}
        port: {port}
        protocol: rest
        enforcement: enforce
        rules:
          - allow:
              method: POST
              path: /v1/actions/send_email
          - allow:
              method: POST
              path: /v1/actions/create_event
          - allow:
              method: POST
              path: /v1/actions/llm_complete
    binaries:
      - path: /usr/local/bin/hermes
      - path: /usr/bin/python3.11
      - path: /opt/hermes/.venv/bin/python
"#,
        host = gateway.host,
        port = gateway.port,
    ))
}

fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> ConsentLedger {
        ConsentLedger {
            client_id: "synthetic-client-001".to_owned(),
            consent_document_ids: vec!["synthetic-consent-001".to_owned()],
        }
    }

    #[test]
    fn output_retains_nemoclaw_fields_and_only_gateway_egress() {
        let yaml = compile_boundary(
            &ledger(),
            &GatewayEndpoint {
                host: "pru-gateway.local".to_owned(),
                port: 443,
            },
        )
        .unwrap();
        assert!(yaml.contains("managed-startup-ca-bundle.pem"));
        assert!(yaml.contains("run_as_user: sandbox"));
        assert!(yaml.contains("compatibility: best_effort"));
        assert!(yaml.contains("protocol: rest"));
        assert!(!yaml.contains("protocol: graphql"));
        assert_eq!(yaml.matches("host:").count(), 1);
        assert!(yaml.contains("host: pru-gateway.local"));
        assert!(!yaml.contains("api.tokenfactory.nebius.com"));
    }

    #[test]
    fn unsafe_hostname_is_rejected_before_yaml_generation() {
        let error = compile_boundary(
            &ledger(),
            &GatewayEndpoint {
                host: "safe.example\n  exfil: true".to_owned(),
                port: 443,
            },
        )
        .unwrap_err();
        assert!(matches!(error, BoundaryError::InvalidGatewayHost(_)));
    }
}
