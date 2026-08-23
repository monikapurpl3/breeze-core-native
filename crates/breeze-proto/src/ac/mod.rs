//! Air-conditioner specifics: the enums, the commands we send, and the reports
//! we get back. Everything below this module is transport and knows nothing
//! about appliances.

pub mod capabilities;
pub mod command;
pub mod response;
pub mod types;
