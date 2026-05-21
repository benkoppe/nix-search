use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use nix_search_config::{AppConfig, ProducerConfig, RefConfig, SourceConfig, SourceKind};
use nix_search_core::{
    ArtifactKind, CommonDoc, SearchDocument, SourceLinkConfig, SourceLinkResolver,
};
use nix_search_index::{
    IndexGenerationManifest, IndexStore, IndexTargetManifest, SearchHit, SearchIndex,
    SearchOptions, SearchScope,
};
use nix_search_source::{
    ChannelPackagesJsonProducer, Consumer, EvalModulesProducer, ExistingFileProducer,
    NixBuildOptionsJsonProducer, OptionsJsonConsumer, PackagesJsonConsumer, ProduceRequest,
    ProducedArtifact, Producer,
};
use nix_search_store::{ArtifactRef, ArtifactStore};

#[derive(Debug, Parser)]
#[command(name = "nix-search")]
#[command(about = "Search Nix packages and options")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate and display configuration.
    CheckConfig(ConfigArgs),

    /// Produce artifacts and build indexes for configured refs.
    Update(SelectionArgs),

    /// Search configured indexes.
    Search(SearchArgs),

    /// Serve the web UI.
    Serve(ConfigArgs),

    /// Debug artifact production and metadata.
    Artifact {
        #[command(subcommand)]
        command: ArtifactCommand,
    },

    /// Debug index building and inspection.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ArtifactCommand {
    /// Produce raw artifacts only, without indexing.
    Produce(SelectionArgs),

    /// Inspect artifact metadata.
    Inspect(SelectionArgs),
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    /// Rebuild the current index from exactly the selected refs.
    Rebuild(SelectionArgs),

    /// Inspect the current published index generation.
    Inspect(ConfigArgs),
}

#[derive(Debug, Args)]
struct ConfigArgs {
    /// Path to config file. If omitted, only defaults and env vars are loaded.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct SelectionArgs {
    /// Path to config file.
    #[arg(long)]
    config: PathBuf,

    /// Restrict to one source.
    #[arg(long)]
    source: Option<String>,

    /// Restrict to one ref.
    #[arg(long = "ref")]
    ref_id: Option<String>,
}

#[derive(Debug, Args)]
struct SearchArgs {
    /// Search query.
    query: String,

    /// Path to config file.
    #[arg(long)]
    config: PathBuf,

    /// Restrict to one source.
    #[arg(long)]
    source: Option<String>,

    /// Restrict to one ref. Requires --source.
    #[arg(long = "ref")]
    ref_id: Option<String>,

    /// Maximum number of results.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Debug, Clone)]
struct TargetRef {
    source_id: String,
    source_kind: SourceKind,
    ref_config: RefConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TargetKey {
    source: String,
    ref_id: String,
}

impl TargetKey {
    fn new(source: impl Into<String>, ref_id: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            ref_id: ref_id.into(),
        }
    }
}

impl From<&TargetRef> for TargetKey {
    fn from(target: &TargetRef) -> Self {
        Self::new(target.source_id.clone(), target.ref_config.id.clone())
    }
}

impl From<&IndexTargetManifest> for TargetKey {
    fn from(target: &IndexTargetManifest) -> Self {
        Self::new(target.source.clone(), target.ref_id.clone())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::CheckConfig(args) => check_config(args),
        Command::Update(args) => update(args).await,
        Command::Search(args) => search(args),
        Command::Serve(args) => serve(args).await,
        Command::Artifact { command } => match command {
            ArtifactCommand::Produce(args) => artifact_produce(args).await,
            ArtifactCommand::Inspect(args) => artifact_inspect(args).await,
        },
        Command::Index { command } => match command {
            IndexCommand::Rebuild(args) => index_rebuild(args).await,
            IndexCommand::Inspect(args) => index_inspect(args),
        },
    }
}

fn check_config(args: ConfigArgs) -> Result<()> {
    let config = AppConfig::load(args.config.as_deref()).context("configuration check failed")?;

    println!("configuration is valid");
    println!("artifact_url = {}", config.data.artifact_url);
    println!("index_dir = {}", config.data.index_dir.display());
    println!("listen = {}", config.server.listen);
    println!("sources = {}", config.sources.len());

    for (source_id, source) in &config.sources {
        print_source(source_id, source);
    }

    Ok(())
}

async fn update(args: SelectionArgs) -> Result<()> {
    let config = load_required_config(&args.config)?;
    let store = artifact_store_from_config(&config)?;
    let selected_targets = select_targets(&config, &args)?;

    if selected_targets.is_empty() {
        bail!("no refs matched selection");
    }

    let index_store = IndexStore::new(&config.data.index_dir);

    let mut included_targets = current_manifest_targets(&config, &index_store)?;
    let selected_keys: BTreeSet<TargetKey> = selected_targets.iter().map(TargetKey::from).collect();

    for target in selected_targets {
        included_targets.insert(TargetKey::from(&target), target);
    }

    if included_targets.is_empty() {
        bail!("no refs available to index");
    }

    build_and_publish_generation(
        &index_store,
        &store,
        included_targets.into_values().collect(),
        &selected_keys,
    )
    .await
}

async fn artifact_produce(args: SelectionArgs) -> Result<()> {
    let config = load_required_config(&args.config)?;
    let store = artifact_store_from_config(&config)?;
    let targets = select_targets(&config, &args)?;

    if targets.is_empty() {
        bail!("no refs matched selection");
    }

    for target in targets {
        let produced = produce_target(&store, &target).await?;
        print_artifact_metadata(&produced);
    }

    Ok(())
}

async fn artifact_inspect(args: SelectionArgs) -> Result<()> {
    let config = load_required_config(&args.config)?;
    let store = artifact_store_from_config(&config)?;
    let targets = select_targets(&config, &args)?;

    if targets.is_empty() {
        bail!("no refs matched selection");
    }

    for target in targets {
        let artifact_ref = latest_artifact_ref_for_target(&target);
        let metadata = store.get_metadata(&artifact_ref).await.with_context(|| {
            format!(
                "failed to read metadata for {}/{}",
                target.source_id, target.ref_config.id
            )
        })?;

        println!("artifact");
        println!("  source = {}", metadata.source);
        println!("  ref = {}", metadata.ref_id);
        println!("  kind = {:?}", metadata.kind);
        println!("  producer = {}", metadata.producer);
        println!(
            "  revision = {}",
            metadata.revision.as_deref().unwrap_or("-")
        );
        println!(
            "  upstream = {}",
            metadata.source_url.as_deref().unwrap_or("-")
        );
        println!("  hash = {}", metadata.content_hash);
        println!("  size = {}", metadata.size_bytes);
        println!("  produced_at = {}", metadata.produced_at);

        for warning in &metadata.warnings {
            println!("  warning = {warning}");
        }
    }

    Ok(())
}

async fn index_rebuild(args: SelectionArgs) -> Result<()> {
    let config = load_required_config(&args.config)?;
    let store = artifact_store_from_config(&config)?;
    let targets = select_targets(&config, &args)?;

    if targets.is_empty() {
        bail!("no refs matched selection");
    }

    let index_store = IndexStore::new(&config.data.index_dir);
    let refresh_keys: BTreeSet<TargetKey> = targets.iter().map(TargetKey::from).collect();

    build_and_publish_generation(&index_store, &store, targets, &refresh_keys).await
}

async fn build_and_publish_generation(
    index_store: &IndexStore,
    artifact_store: &ArtifactStore,
    targets: Vec<TargetRef>,
    refresh_keys: &BTreeSet<TargetKey>,
) -> Result<()> {
    let generation_path = index_store.create_generation_path()?;

    let index = SearchIndex::create_or_replace(&generation_path)?;
    let mut writer = index.writer()?;

    let mut total_documents = 0usize;
    let mut manifest_targets = Vec::new();

    for target in targets {
        let key = TargetKey::from(&target);

        let produced = if refresh_keys.contains(&key) {
            produce_target(artifact_store, &target).await?
        } else {
            produced_from_existing_artifact(artifact_store, &target).await?
        };

        let documents = consume_target(artifact_store, &target, &produced).await?;

        for document in &documents {
            writer.add_document(document)?;
        }

        total_documents += documents.len();

        manifest_targets.push(IndexTargetManifest {
            source: target.source_id.clone(),
            ref_id: target.ref_config.id.clone(),
            artifact_kind: produced.artifact_ref.kind,
            document_count: documents.len(),
            artifact_hash: Some(produced.metadata.content_hash.clone()),
            revision: produced.metadata.revision.clone(),
        });

        println!(
            "{} {} documents: {}/{}",
            if refresh_keys.contains(&key) {
                "refreshed"
            } else {
                "retained"
            },
            documents.len(),
            target.source_id,
            target.ref_config.id
        );
    }

    writer.commit()?;

    let manifest = IndexGenerationManifest::new(total_documents, manifest_targets);
    index_store.write_manifest(&generation_path, &manifest)?;
    index_store.publish(&generation_path)?;

    println!("published index generation");
    println!("  generation = {}", generation_path.display());
    println!("  documents = {total_documents}");

    Ok(())
}

fn index_inspect(args: ConfigArgs) -> Result<()> {
    let config = AppConfig::load(args.config.as_deref()).context("failed to load config")?;
    let index_store = IndexStore::new(&config.data.index_dir);

    let current_path = index_store.current_path()?;
    let manifest = index_store.current_manifest()?;

    println!("current index");
    println!("  path = {}", current_path.display());
    println!("  schema_version = {}", manifest.schema_version);
    println!("  generated_at = {}", manifest.generated_at);
    println!("  documents = {}", manifest.document_count);
    println!("  targets = {}", manifest.targets.len());

    for target in manifest.targets {
        println!(
            "    {}/{} {:?} documents={}",
            target.source, target.ref_id, target.artifact_kind, target.document_count
        );

        if let Some(revision) = target.revision {
            println!("      revision = {revision}");
        }

        if let Some(hash) = target.artifact_hash {
            println!("      artifact_hash = {hash}");
        }
    }

    Ok(())
}

fn search(args: SearchArgs) -> Result<()> {
    let config = load_required_config(&args.config)?;

    let index_store = IndexStore::new(&config.data.index_dir);
    let current_path = index_store.current_path()?;

    let index = SearchIndex::open(&current_path)
        .with_context(|| format!("failed to open current index {}", current_path.display()))?;

    let scopes = config
        .resolve_search_scopes(args.source.as_deref(), args.ref_id.as_deref())
        .context("failed to resolve search scope")?
        .into_iter()
        .map(|scope| SearchScope {
            source: scope.source,
            ref_id: scope.ref_id,
        })
        .collect();

    let hits = index.search(SearchOptions {
        query: args.query,
        limit: args.limit,
        scopes,
        ..Default::default()
    })?;

    for hit in hits.hits {
        print_search_hit(&config, hit);
    }

    Ok(())
}

async fn serve(args: ConfigArgs) -> Result<()> {
    let config = AppConfig::load(args.config.as_deref()).context("failed to load config")?;

    nix_search_web::serve(config).await
}

async fn produce_target(store: &ArtifactStore, target: &TargetRef) -> Result<ProducedArtifact> {
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

async fn produced_from_existing_artifact(
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

async fn consume_target(
    store: &ArtifactStore,
    target: &TargetRef,
    produced: &ProducedArtifact,
) -> Result<Vec<SearchDocument>> {
    match (target.source_kind, produced.artifact_ref.kind) {
        (SourceKind::Options | SourceKind::Mixed, ArtifactKind::OptionsJson) => {
            let consumer = OptionsJsonConsumer;

            consumer.consume(store, produced).await.with_context(|| {
                format!(
                    "failed to consume options artifact for {}/{}",
                    target.source_id, target.ref_config.id
                )
            })
        }

        (SourceKind::Packages | SourceKind::Mixed, ArtifactKind::PackagesJson) => {
            let consumer = PackagesJsonConsumer;

            consumer.consume(store, produced).await.with_context(|| {
                format!(
                    "failed to consume packages artifact for {}/{}",
                    target.source_id, target.ref_config.id
                )
            })
        }

        (kind, artifact) => bail!(
            "no consumer implemented for source kind {:?} and artifact kind {:?}",
            kind,
            artifact
        ),
    }
}

fn load_required_config(path: &Path) -> Result<AppConfig> {
    AppConfig::load(Some(path)).with_context(|| format!("failed to load {}", path.display()))
}

fn artifact_store_from_config(config: &AppConfig) -> Result<ArtifactStore> {
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

fn latest_artifact_ref_for_target(target: &TargetRef) -> ArtifactRef {
    ArtifactRef::latest(
        target.source_id.clone(),
        target.ref_config.id.clone(),
        artifact_kind_for_producer(&target.ref_config.producer),
    )
}

fn artifact_kind_for_producer(producer: &ProducerConfig) -> ArtifactKind {
    match producer {
        ProducerConfig::ExistingFile { artifact, .. } => *artifact,
        ProducerConfig::ChannelPackagesJson { .. } => ArtifactKind::PackagesJson,
        ProducerConfig::NixBuildOptionsJson { .. } => ArtifactKind::OptionsJson,
        ProducerConfig::EvalModules { .. } => ArtifactKind::OptionsJson,
        ProducerConfig::Download { artifact, .. } => *artifact,
        ProducerConfig::CustomCommand { artifact, .. } => *artifact,
        ProducerConfig::FlakeOutput { .. } => ArtifactKind::FlakeInfoJson,
    }
}

fn select_targets(config: &AppConfig, selection: &SelectionArgs) -> Result<Vec<TargetRef>> {
    let mut targets = Vec::new();

    for (source_id, source) in &config.sources {
        if selection
            .source
            .as_ref()
            .is_some_and(|selected| selected != source_id)
        {
            continue;
        }

        collect_source_targets(source_id, source, selection, &mut targets);
    }

    if let Some(source_id) = &selection.source
        && !config.sources.contains_key(source_id)
    {
        bail!("unknown source {source_id:?}");
    }

    Ok(targets)
}

fn collect_source_targets(
    source_id: &str,
    source: &SourceConfig,
    selection: &SelectionArgs,
    targets: &mut Vec<TargetRef>,
) {
    for ref_config in &source.refs {
        if selection
            .ref_id
            .as_ref()
            .is_some_and(|selected| selected != &ref_config.id)
        {
            continue;
        }

        targets.push(TargetRef {
            source_id: source_id.to_owned(),
            source_kind: source.kind,
            ref_config: ref_config.clone(),
        });
    }
}

fn print_source(source_id: &str, source: &SourceConfig) {
    let name = source.name.as_deref().unwrap_or(source_id);
    println!("  source {source_id}: {name} ({:?})", source.kind);

    for ref_config in &source.refs {
        println!(
            "    ref {}: producer={:?}",
            ref_config.id,
            ref_config.producer.kind()
        );
    }
}

fn print_artifact_metadata(produced: &ProducedArtifact) {
    println!("produced artifact");
    println!("  source = {}", produced.metadata.source);
    println!("  ref = {}", produced.metadata.ref_id);
    println!("  kind = {:?}", produced.metadata.kind);
    println!("  producer = {}", produced.metadata.producer);
    println!(
        "  revision = {}",
        produced.metadata.revision.as_deref().unwrap_or("-")
    );
    println!(
        "  upstream = {}",
        produced.metadata.source_url.as_deref().unwrap_or("-")
    );
    println!("  hash = {}", produced.metadata.content_hash);
    println!("  size = {}", produced.metadata.size_bytes);

    for warning in &produced.metadata.warnings {
        println!("warning = {warning}");
    }
}

fn print_search_hit(config: &AppConfig, hit: SearchHit) {
    let common = hit.document.common().clone();

    println!(
        "{score:.3}  {kind}  {source}/{ref_id}  {name}",
        score = hit.score,
        kind = common.kind.as_str(),
        source = common.source,
        ref_id = common.ref_id,
        name = common.name,
    );

    if !common.name_parts.groups.is_empty() {
        println!("       groups: {}", common.name_parts.groups.join(", "));
    }

    let resolver = source_link_config_for_document(config, &common)
        .map(|source_links| SourceLinkResolver::new(source_links, common.revision.as_deref()));

    match hit.document {
        SearchDocument::Option(option) => {
            if let Some(description) = option.description {
                let summary = description.lines().next().unwrap_or("").trim();

                if !summary.is_empty() {
                    println!("       {summary}");
                }
            }

            if let Some(resolver) = resolver {
                for declaration in &option.declarations {
                    if let Some(url) = resolver.resolve_declaration(declaration) {
                        println!("       source: {url}");
                        break;
                    }
                }
            }
        }
        SearchDocument::Package(package) => {
            let mut details = Vec::new();

            if let Some(pname) = package.pname {
                details.push(pname);
            }

            if let Some(version) = package.version {
                details.push(version);
            }

            if !details.is_empty() {
                println!("       {}", details.join(" "));
            }

            if let Some(description) = package.description {
                let summary = description.lines().next().unwrap_or("").trim();

                if !summary.is_empty() {
                    println!("       {summary}");
                }
            }

            if let (Some(resolver), Some(position)) = (resolver, package.position.as_deref())
                && let Some(url) = resolver.resolve_package_position(position)
            {
                println!("       source: {url}");
            }
        }
    }
}

fn source_link_config_for_document<'a>(
    config: &'a AppConfig,
    common: &CommonDoc,
) -> Option<&'a SourceLinkConfig> {
    let source = config.sources.get(&common.source)?;

    let ref_config = source
        .refs
        .iter()
        .find(|ref_config| ref_config.id == common.ref_id)?;

    ref_config.source_links.as_ref()
}

fn current_manifest_targets(
    config: &AppConfig,
    index_store: &IndexStore,
) -> Result<BTreeMap<TargetKey, TargetRef>> {
    let Some(manifest) = index_store.try_current_manifest()? else {
        return Ok(BTreeMap::new());
    };

    let mut targets = BTreeMap::new();

    for manifest_target in &manifest.targets {
        let target = resolve_manifest_target(config, manifest_target)?;
        targets.insert(TargetKey::from(manifest_target), target);
    }

    Ok(targets)
}

fn resolve_manifest_target(
    config: &AppConfig,
    manifest_target: &IndexTargetManifest,
) -> Result<TargetRef> {
    let source = config
        .sources
        .get(&manifest_target.source)
        .with_context(|| {
            format!(
                "current index manifest contains unknown source {:?}",
                manifest_target.source
            )
        })?;

    let ref_config = source
        .refs
        .iter()
        .find(|ref_config| ref_config.id == manifest_target.ref_id)
        .with_context(|| {
            format!(
                "current index manifest contains unknown ref {:?} in source {:?}",
                manifest_target.ref_id, manifest_target.source
            )
        })?;

    Ok(TargetRef {
        source_id: manifest_target.source.clone(),
        source_kind: source.kind,
        ref_config: ref_config.clone(),
    })
}
