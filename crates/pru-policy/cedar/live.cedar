// A use is permitted only by a matching, unexpired use consent.
permit (
    principal is PruAgent,
    action == Action::"use",
    resource is DataItem
)
when {
    context.consent_kind == "use" &&
    context.consent_client_id == resource.client_id &&
    context.consent_purpose == context.requested_purpose &&
    context.requested_purpose != "" &&
    context.consent_expires_at > context.now
};

// A disclosure is permitted only by the same client's unexpired disclosure
// consent naming the exact recipient and purpose.
permit (
    principal is PruAgent,
    action == Action::"disclose",
    resource is DataItem
)
when {
    context.consent_kind == "disclose" &&
    context.consent_client_id == resource.client_id &&
    context.consent_recipient == resource.recipient &&
    context.consent_purpose == context.requested_purpose &&
    context.requested_purpose != "" &&
    context.consent_expires_at > context.now
};

// Local is the only destination label permitted to carry an SSN.
forbid (
    principal,
    action,
    resource is DataItem
)
when {
    resource.contains_ssn && resource.destination_region != "local"
};
