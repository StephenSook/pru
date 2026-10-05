// Reference policy R permits use generally, but never permits an SSN to leave
// local processing. The live policy is intentionally stricter for use.
permit (
    principal is PruAgent,
    action == Action::"use",
    resource is DataItem
);

// Reference policy R requires disclosure consent to match the client,
// recipient and purpose, and requires it to be unexpired.
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

forbid (
    principal,
    action,
    resource is DataItem
)
when {
    resource.contains_ssn && resource.destination_region != "local"
};
