pub mod balance;
pub mod client;
pub(super) mod glue;
pub mod h1;
pub mod insert_target;
pub mod normalize_uri;
pub mod orig_proto;
pub mod router;
pub mod settings;
pub mod upgrade;

pub use self::client::{Client, Error as ClientError};
pub use self::glue::HttpBody as Body;
pub use self::settings::Settings;
