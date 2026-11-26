//! Where the index's data comes from: repo chunks, Python text rules (`py`),
//! JSON files that are never half-written (`store`), failure kinds (`Kind`), the GitHub connection, raw data and
//! documents (`github`), Slack (`slack`), chunks of documents outside the repo (`doc`), Jira and Confluence
//! (`atlassian`), Dropbox (`dropbox`), Notion (`notion`), past meetings (`meeting`), this Mac's note folders (`notes`),
//! gathering chunks from every source (`gather`), and loopback address checks and network error text (`net`). Every
//! source produces chunks of the same shape (`RawChunk`).
//! Test fakes (`testing`) are visible only with the `testing` feature on.

pub mod asana;
pub mod atlassian;
pub mod doc;
pub mod dropbox;
pub mod error;
pub mod extract;
pub mod gather;
pub mod gdrive;
pub mod github;
pub mod linear;
pub mod meeting;
pub mod net;
pub mod notes;
pub mod notion;
pub mod py;
pub mod repo;
pub mod slack;
pub mod store;
#[cfg(feature = "testing")]
pub mod testing;

pub use error::{Kind, SourceError};
pub use net::{is_loopback, net_error};
pub use store::Stored;
