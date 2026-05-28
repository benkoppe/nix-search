mod manifest;
mod query;
mod ranking;
mod schema;
mod search;
mod store;
mod tokenize;
mod writer;

const WRITER_MEMORY_BYTES: usize = 50_000_000;

pub use manifest::{IndexGenerationManifest, IndexTargetManifest};
pub use schema::INDEX_SCHEMA_VERSION;
pub use search::{
    EntryLookup, EntryLookupResult, SearchHit, SearchIndex, SearchOptions, SearchResult,
    SearchScope,
};
pub use store::IndexStore;
pub use writer::SearchIndexWriter;
