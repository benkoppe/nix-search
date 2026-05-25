use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use nix_search_config::{AppConfig, ProducerConfig};
use nix_search_core::ArtifactKind;
use nix_search_source::{
    ChannelOptionsJsonProducer, ChannelPackagesJsonProducer, EvalModulesProducer,
    ExistingFileProducer, NixBuildOptionsJsonProducer, ProduceRequest, ProducedArtifact, Producer,
};
use nix_search_store::{ArtifactRef, ArtifactStore};

use crate::targets::TargetRef;

pub async fn produce_target(store: &ArtifactStore, target: &TargetRef) -> Result<ProducedArtifact> {
    let request = ProduceRequest {
        source: target.source_id.clone(),
        ref_id: target.ref_config.id.clone(),
    };

    match &target.ref_config.producer {
        ProducerConfig::ExistingFile { path, artifact } => {
            let producer = ExistingFileProducer::new(path, *artifact);

            producer.produce(store, &request).await.with_context(|| {
                format!(
                    "failed to produce artifact for {}/{}",
                    target.source_id, target.ref_config.id
                )
            })
        }

        ProducerConfig::NixBuildOptionsJson {
            source_ref,
            attribute,
            import_path,
            output_path,
        } => {
            let producer =
                NixBuildOptionsJsonProducer::new(source_ref, attribute, import_path, output_path);

            producer.produce(store, &request).await.with_context(|| {
                format!(
                    "failed to produce Nix-built options artifact for {}/{}",
                    target.source_id, target.ref_config.id
                )
            })
        }

        ProducerConfig::ChannelPackagesJson { channel, url } => {
            let producer = ChannelPackagesJsonProducer::new(channel, url.clone());

            producer.produce(store, &request).await.with_context(|| {
                format!(
                    "failed to produce channel packages artifact for {}/{}",
                    target.source_id, target.ref_config.id
                )
            })
        }

        ProducerConfig::ChannelOptionsJson { channel, url } => {
            let producer = ChannelOptionsJsonProducer::new(channel, url.clone());

            producer.produce(store, &request).await.with_context(|| {
                format!(
                    "failed to produce channel options artifact for {}/{}",
                    target.source_id, target.ref_config.id
                )
            })
        }

        ProducerConfig::EvalModules {
            source_ref,
            modules_attr,
            url_prefix,
        } => {
            let producer = EvalModulesProducer::new(source_ref, modules_attr, url_prefix.clone());

            producer.produce(store, &request).await.with_context(|| {
                format!(
                    "failed to produce eval-modules options artifact for {}/{}",
                    target.source_id, target.ref_config.id
                )
            })
        }

        unsupported => bail!(
            "producer {:?} is configured but not implemented yet",
            unsupported.kind()
        ),
    }
}

pub async fn produced_from_existing_artifact(
    store: &ArtifactStore,
    target: &TargetRef,
) -> Result<ProducedArtifact> {
    let artifact_ref = latest_artifact_ref_for_target(target);
    let metadata = store.get_metadata(&artifact_ref).await.with_context(|| {
        format!(
            "failed to read artifact metadata for retained target {}/{}",
            target.source_id, target.ref_config.id
        )
    })?;

    Ok(ProducedArtifact {
        artifact_ref,
        metadata,
    })
}

pub fn latest_artifact_ref_for_target(target: &TargetRef) -> ArtifactRef {
    ArtifactRef::latest(
        target.source_id.clone(),
        target.ref_config.id.clone(),
        artifact_kind_for_producer(&target.ref_config.producer),
    )
}

pub fn artifact_kind_for_producer(producer: &ProducerConfig) -> ArtifactKind {
    match producer {
        ProducerConfig::ExistingFile { artifact, .. } => *artifact,
        ProducerConfig::ChannelPackagesJson { .. } => ArtifactKind::PackagesJson,
        ProducerConfig::ChannelOptionsJson { .. } => ArtifactKind::OptionsJson,
        ProducerConfig::NixBuildOptionsJson { .. } => ArtifactKind::OptionsJson,
        ProducerConfig::EvalModules { .. } => ArtifactKind::OptionsJson,
        ProducerConfig::Download { artifact, .. } => *artifact,
        ProducerConfig::CustomCommand { artifact, .. } => *artifact,
        ProducerConfig::FlakeOutput { .. } => ArtifactKind::FlakeInfoJson,
    }
}

pub fn artifact_store_from_config(config: &AppConfig) -> Result<ArtifactStore> {
    let artifact_path = file_url_to_path(&config.data.artifact_url)?;

    ArtifactStore::local(&artifact_path).with_context(|| {
        format!(
            "failed to open artifact store from {}",
            config.data.artifact_url
        )
    })
}

fn file_url_to_path(url: &str) -> Result<PathBuf> {
    let path = url.strip_prefix("file://").with_context(|| {
        format!("only file:// artifact_url values are currently supported: {url}")
    })?;

    if path.trim().is_empty() {
        bail!("file:// artifact_url must include a path");
    }

    Ok(PathBuf::from(path))
}
