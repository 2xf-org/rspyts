use rspyts::bridge;

// Per-item wire renames were removed on purpose: the camelCase convention
// IS the contract. `rename` must fail as an ordinary unknown argument.
#[bridge]
pub struct AuditRecord {
    #[bridge(rename = "access.login.succeeded")]
    pub login_event: String,
}

fn main() {}
