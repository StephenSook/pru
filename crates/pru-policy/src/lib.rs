//! Cedar authorization for Pru egress decisions.

use std::{collections::HashSet, str::FromStr};

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityUid, PolicySet, Request,
    RestrictedExpression, Schema,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The principal asking the gateway to perform an action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPrincipal {
    /// The expected Pru agent principal.
    PruAgent,
    /// Any principal which is not the Pru agent.
    Other(String),
}

/// An action decided by Cedar.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressAction {
    /// Use return information within the practice.
    Use,
    /// Disclose return information to a named recipient.
    Disclose,
}

impl EgressAction {
    fn cedar_id(self) -> &'static str {
        match self {
            Self::Use => "use",
            Self::Disclose => "disclose",
        }
    }
}

/// Labels attached to the exact item leaving through the gateway.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DataResource {
    /// Stable item identifier used only for the decision and ledger.
    pub id: String,
    /// Client compartment which owns the item.
    pub client_id: String,
    /// Whether the detector found an SSN span.
    pub contains_ssn: bool,
    /// `local` or a non-local destination label.
    pub destination_region: String,
    /// Exact recipient for a disclosure. Empty for a use.
    pub recipient: String,
}

/// Facts extracted only from already verified consent tokens.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerifiedConsentFacts {
    /// Client identifier in the consent.
    pub client_id: String,
    /// `use` or `disclose`.
    pub kind: String,
    /// Exact recipient. Empty for a use consent.
    pub recipient: String,
    /// Exact stated purpose.
    pub purpose: String,
    /// Token expiry instant.
    pub expires_at: DateTime<Utc>,
}

/// Complete input to one Cedar authorization decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EgressRequest {
    /// Calling principal.
    pub principal: EgressPrincipal,
    /// Requested egress action.
    pub action: EgressAction,
    /// Data item labels.
    pub resource: DataResource,
    /// Purpose requested for this action.
    pub purpose: String,
    /// Facts from all verified, non-revoked consent tokens presented.
    pub consent_facts: Vec<VerifiedConsentFacts>,
    /// Decision time supplied by the gateway.
    pub now: DateTime<Utc>,
}

/// Result safe to append to an egress ledger.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EgressDecision {
    /// Cedar's final allow or deny result.
    pub allowed: bool,
    /// Stable, non-sensitive explanation codes.
    pub diagnostics: Vec<String>,
}

/// A failure to construct or evaluate the Cedar request.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// A bundled Cedar source file did not parse.
    #[error("failed to parse bundled Cedar source: {0}")]
    CedarSource(String),
    /// An incoming field could not be represented safely in Cedar.
    #[error("failed to construct Cedar request: {0}")]
    Request(String),
}

/// Cedar-backed authorizer for gateway egress.
#[derive(Clone, Debug)]
pub struct EgressAuthorizer {
    schema: Schema,
    policies: PolicySet,
}

impl EgressAuthorizer {
    /// Parse and validate the bundled live policy set.
    pub fn new() -> Result<Self, PolicyError> {
        let schema = Schema::from_cedarschema_str(include_str!("../cedar/schema.cedarschema"))
            .map_err(|error| PolicyError::CedarSource(error.to_string()))?
            .0;
        let policies = PolicySet::from_str(include_str!("../cedar/live.cedar"))
            .map_err(|error| PolicyError::CedarSource(error.to_string()))?;
        let validation = cedar_policy::Validator::new(schema.clone())
            .validate(&policies, cedar_policy::ValidationMode::Strict);
        if !validation.validation_passed() {
            return Err(PolicyError::CedarSource(validation.to_string()));
        }
        Ok(Self { schema, policies })
    }

    /// Decide an action. Each verified consent is evaluated separately, and
    /// one matching fact is sufficient. A Cedar forbid overrides its permit.
    pub fn authorize(&self, input: &EgressRequest) -> Result<EgressDecision, PolicyError> {
        if input.principal != EgressPrincipal::PruAgent {
            return Ok(EgressDecision {
                allowed: false,
                diagnostics: vec!["principal_not_pru_agent".to_owned()],
            });
        }

        let entities = self.entities_for(&input.resource)?;
        let facts = if input.consent_facts.is_empty() {
            vec![VerifiedConsentFacts {
                client_id: String::new(),
                kind: String::new(),
                recipient: String::new(),
                purpose: String::new(),
                expires_at: DateTime::UNIX_EPOCH,
            }]
        } else {
            input.consent_facts.clone()
        };

        for fact in facts {
            let request = self.cedar_request(input, &fact)?;
            let response = Authorizer::new().is_authorized(&request, &self.policies, &entities);
            if response.decision() == Decision::Allow {
                return Ok(EgressDecision {
                    allowed: true,
                    diagnostics: vec!["cedar_allow".to_owned()],
                });
            }
        }

        let diagnostic =
            if input.resource.contains_ssn && input.resource.destination_region != "local" {
                "ssn_requires_local_destination"
            } else if input.consent_facts.is_empty() {
                "matching_consent_required"
            } else {
                "consent_facts_do_not_match"
            };
        Ok(EgressDecision {
            allowed: false,
            diagnostics: vec![diagnostic.to_owned()],
        })
    }

    fn entities_for(&self, resource: &DataResource) -> Result<Entities, PolicyError> {
        let uid = entity_uid("DataItem", &resource.id)?;
        let attrs = [
            ("client_id", cedar_string(&resource.client_id)?),
            (
                "contains_ssn",
                RestrictedExpression::from_str(if resource.contains_ssn {
                    "true"
                } else {
                    "false"
                })
                .map_err(|error| PolicyError::Request(error.to_string()))?,
            ),
            (
                "destination_region",
                cedar_string(&resource.destination_region)?,
            ),
            ("recipient", cedar_string(&resource.recipient)?),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
        let entity = Entity::new(uid, attrs, HashSet::new())
            .map_err(|error| PolicyError::Request(error.to_string()))?;
        Entities::from_entities([entity], Some(&self.schema))
            .map_err(|error| PolicyError::Request(error.to_string()))
    }

    fn cedar_request(
        &self,
        input: &EgressRequest,
        fact: &VerifiedConsentFacts,
    ) -> Result<Request, PolicyError> {
        let context = Context::from_pairs(
            [
                ("consent_client_id", cedar_string(&fact.client_id)?),
                ("consent_kind", cedar_string(&fact.kind)?),
                ("consent_recipient", cedar_string(&fact.recipient)?),
                ("consent_purpose", cedar_string(&fact.purpose)?),
                (
                    "consent_expires_at",
                    cedar_long(fact.expires_at.timestamp())?,
                ),
                ("requested_purpose", cedar_string(&input.purpose)?),
                ("now", cedar_long(input.now.timestamp())?),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value)),
        )
        .map_err(|error| PolicyError::Request(error.to_string()))?;

        Request::new(
            entity_uid("PruAgent", "pru")?,
            entity_uid("Action", input.action.cedar_id())?,
            entity_uid("DataItem", &input.resource.id)?,
            context,
            Some(&self.schema),
        )
        .map_err(|error| PolicyError::Request(error.to_string()))
    }
}

impl Default for EgressAuthorizer {
    fn default() -> Self {
        Self::new().expect("bundled Cedar policy must parse and validate")
    }
}

fn entity_uid(kind: &str, id: &str) -> Result<EntityUid, PolicyError> {
    format!("{kind}::{id:?}")
        .parse()
        .map_err(|error: cedar_policy::ParseErrors| PolicyError::Request(error.to_string()))
}

fn cedar_string(value: &str) -> Result<RestrictedExpression, PolicyError> {
    let encoded =
        serde_json::to_string(value).map_err(|error| PolicyError::Request(error.to_string()))?;
    RestrictedExpression::from_str(&encoded)
        .map_err(|error| PolicyError::Request(error.to_string()))
}

fn cedar_long(value: i64) -> Result<RestrictedExpression, PolicyError> {
    RestrictedExpression::from_str(&value.to_string())
        .map_err(|error| PolicyError::Request(error.to_string()))
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn request(action: EgressAction, contains_ssn: bool, destination: &str) -> EgressRequest {
        let now = Utc::now();
        EgressRequest {
            principal: EgressPrincipal::PruAgent,
            action,
            resource: DataResource {
                id: "synthetic-document-1".to_owned(),
                client_id: "synthetic-client-1".to_owned(),
                contains_ssn,
                destination_region: destination.to_owned(),
                recipient: "synthetic-recipient.example".to_owned(),
            },
            purpose: "synthetic tax document review".to_owned(),
            consent_facts: vec![VerifiedConsentFacts {
                client_id: "synthetic-client-1".to_owned(),
                kind: action.cedar_id().to_owned(),
                recipient: "synthetic-recipient.example".to_owned(),
                purpose: "synthetic tax document review".to_owned(),
                expires_at: now + TimeDelta::days(30),
            }],
            now,
        }
    }

    #[test]
    fn matching_disclosure_consent_allows_ssn_free_remote_data() {
        let decision = EgressAuthorizer::new()
            .unwrap()
            .authorize(&request(EgressAction::Disclose, false, "dynamic_remote"))
            .unwrap();
        assert!(decision.allowed);
    }

    #[test]
    fn disclosure_without_consent_is_denied() {
        let mut input = request(EgressAction::Disclose, false, "dynamic_remote");
        input.consent_facts.clear();
        let decision = EgressAuthorizer::new().unwrap().authorize(&input).unwrap();
        assert!(!decision.allowed);
        assert_eq!(decision.diagnostics, ["matching_consent_required"]);
    }

    #[test]
    fn expired_and_exactly_expiring_consents_are_denied() {
        let mut input = request(EgressAction::Use, false, "local");
        input.consent_facts[0].expires_at = input.now;
        assert!(
            !EgressAuthorizer::new()
                .unwrap()
                .authorize(&input)
                .unwrap()
                .allowed
        );
        input.consent_facts[0].expires_at = input.now - TimeDelta::seconds(1);
        assert!(
            !EgressAuthorizer::new()
                .unwrap()
                .authorize(&input)
                .unwrap()
                .allowed
        );
    }

    #[test]
    fn ssn_never_leaves_local_destination() {
        let decision = EgressAuthorizer::new()
            .unwrap()
            .authorize(&request(EgressAction::Disclose, true, "dynamic_remote"))
            .unwrap();
        assert!(!decision.allowed);
        assert_eq!(decision.diagnostics, ["ssn_requires_local_destination"]);
    }

    #[test]
    fn ssn_may_be_used_locally_with_matching_consent() {
        let decision = EgressAuthorizer::new()
            .unwrap()
            .authorize(&request(EgressAction::Use, true, "local"))
            .unwrap();
        assert!(decision.allowed);
    }
}
