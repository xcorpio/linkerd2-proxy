const L5D_ORIG_PROTO: &str = "l5d-orig-proto";

mod downgrade;
mod upgrade;

pub use self::downgrade::Downgrade;
pub use self::upgrade::Upgrade;
