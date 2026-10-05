// Deliberately unsafe policy used only by the negative proof test.
permit (
    principal is PruAgent,
    action == Action::"disclose",
    resource is DataItem
);

forbid (
    principal,
    action,
    resource is DataItem
)
when {
    resource.contains_ssn && resource.destination_region != "local"
};
