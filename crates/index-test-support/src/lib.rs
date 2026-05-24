//! Test helpers that require `nix-search-index`.
//!
//! Kept separate from `nix-search-test-support` so `nix-search-index` can use
//! the base test fixtures in its own tests without creating a dependency cycle.

use std::path::{Path, PathBuf};

use camino::Utf8Path;
use nix_search_core::{ArtifactKind, SearchDocument};
use nix_search_index::{IndexGenerationManifest, IndexStore, IndexTargetManifest, SearchIndex};
use nix_search_test_support::{REF_SMALL, SOURCE_FIXTURES, canonical_documents};

pub fn publish_canonical_index(index_root: &Path) -> PathBuf {
    publish_canonical_mixed_index(index_root)
}

pub fn publish_canonical_index_with_generated_at(
    index_root: &Path,
    generated_at: time::OffsetDateTime,
) -> PathBuf {
    publish_canonical_mixed_index_with_generated_at(index_root, generated_at)
}

pub fn publish_canonical_options_index(index_root: &Path) -> PathBuf {
    publish_canonical_options_index_with_generated_at(index_root, time::OffsetDateTime::now_utc())
}

pub fn publish_canonical_options_index_with_generated_at(
    index_root: &Path,
    generated_at: time::OffsetDateTime,
) -> PathBuf {
    let documents = canonical_documents()
        .into_iter()
        .filter(|document| matches!(document, SearchDocument::Option(_)))
        .collect::<Vec<_>>();
    publish_documents_with_manifest_targets(
        index_root,
        generated_at,
        documents,
        vec![IndexTargetManifest {
            source: SOURCE_FIXTURES.to_owned(),
            ref_id: REF_SMALL.to_owned(),
            artifact_kind: ArtifactKind::OptionsJson,
            document_count: 4,
            artifact_hash: Some("fixture-options-hash".to_owned()),
            revision: Some("fixture-revision".to_owned()),
        }],
    )
}

pub fn publish_canonical_mixed_index(index_root: &Path) -> PathBuf {
    publish_canonical_mixed_index_with_generated_at(index_root, time::OffsetDateTime::now_utc())
}

pub fn publish_canonical_mixed_index_with_generated_at(
    index_root: &Path,
    generated_at: time::OffsetDateTime,
) -> PathBuf {
    let documents = canonical_documents();
    let options_count = documents
        .iter()
        .filter(|document| matches!(document, SearchDocument::Option(_)))
        .count();
    let packages_count = documents
        .iter()
        .filter(|document| matches!(document, SearchDocument::Package(_)))
        .count();

    publish_documents_with_manifest_targets(
        index_root,
        generated_at,
        documents,
        vec![
            IndexTargetManifest {
                source: SOURCE_FIXTURES.to_owned(),
                ref_id: REF_SMALL.to_owned(),
                artifact_kind: ArtifactKind::OptionsJson,
                document_count: options_count,
                artifact_hash: Some("fixture-options-hash".to_owned()),
                revision: Some("fixture-revision".to_owned()),
            },
            IndexTargetManifest {
                source: SOURCE_FIXTURES.to_owned(),
                ref_id: REF_SMALL.to_owned(),
                artifact_kind: ArtifactKind::PackagesJson,
                document_count: packages_count,
                artifact_hash: Some("fixture-packages-hash".to_owned()),
                revision: Some("fixture-revision".to_owned()),
            },
        ],
    )
}

fn publish_documents_with_manifest_targets(
    index_root: &Path,
    generated_at: time::OffsetDateTime,
    documents: Vec<SearchDocument>,
    targets: Vec<IndexTargetManifest>,
) -> PathBuf {
    let index_root = Utf8Path::from_path(index_root).expect("index root must be valid UTF-8");
    let store = IndexStore::new(index_root);
    let generation = store.create_generation_path().unwrap();

    let index = SearchIndex::create_or_replace(&generation).unwrap();
    let mut writer = index.writer().unwrap();

    for doc in &documents {
        writer.add_document(doc).unwrap();
    }

    writer.commit().unwrap();

    let manifest =
        IndexGenerationManifest::with_generated_at(documents.len(), targets, generated_at);

    store.write_manifest(&generation, &manifest).unwrap();
    store.publish(&generation).unwrap();
    store.current_path().unwrap().into_std_path_buf()
}

pub fn assert_canonical_options_manifest_targets(manifest: &IndexGenerationManifest) {
    assert_eq!(manifest.targets.len(), 1);

    let options = &manifest.targets[0];
    assert_eq!(options.artifact_kind, ArtifactKind::OptionsJson);
    assert_eq!(options.source, SOURCE_FIXTURES);
    assert_eq!(options.ref_id, REF_SMALL);
    assert_eq!(options.document_count, 4);
}

pub fn assert_canonical_manifest_targets(manifest: &IndexGenerationManifest) {
    assert_eq!(manifest.targets.len(), 2);

    let options = manifest
        .targets
        .iter()
        .find(|target| target.artifact_kind == ArtifactKind::OptionsJson)
        .expect("expected options target");
    assert_eq!(options.source, SOURCE_FIXTURES);
    assert_eq!(options.ref_id, REF_SMALL);
    assert_eq!(options.document_count, 4);

    let packages = manifest
        .targets
        .iter()
        .find(|target| target.artifact_kind == ArtifactKind::PackagesJson)
        .expect("expected packages target");
    assert_eq!(packages.source, SOURCE_FIXTURES);
    assert_eq!(packages.ref_id, REF_SMALL);
    assert_eq!(packages.document_count, 3);
}
