//! The daemon's state machine and compile flow: a resident project bowl,
//! incremental reconciliation, publication through dsql-generate, and
//! last-outcome replay for no-op batches.

use std::path::{Path, PathBuf};

use bowl::{Bowl, Entity, Mut, Query};

use dsql_core::embedding::{EmbeddedExpressionResolution, ResolvedEmbeddedExpression};
use dsql_core::entities::definition::{DefDecl, DefKind};
use dsql_core::facts::{
    BelongsToFile, Diagnostic, DiagnosticCode, DiagnosticSource, Severity, Span,
};
use dsql_core::source::{
    BelongsToHost, CallsiteSpan, ExtractionResolver, FilePath, ResolutionScope, SourceOffset,
    SourceText, insert_source_scoped,
};
use dsql_generate::publish::{MatchLockMode, PublishedGeneration, sha256_hex};
use dsql_generate::{GenerateError, GenerateOptions};
use dsql_project::Project;

use crate::protocol::{
    DiagnosticLevel, Method, PROTOCOL_VERSION, Request, WireDiagnostic, error_line, json_string,
    render_diagnostics, result_line,
};

pub enum Handled {
    Respond(String),
    Shutdown(String),
}

/// A cached compile outcome, replayed verbatim for no-op batches. The
/// success body carries a `%CHANGED%` placeholder substituted at send.
enum Outcome {
    Success {
        body: String,
        /// Whether this compile actually wrote anything; replays always
        /// send `false`.
        changed: bool,
        published: PublishedGeneration,
    },
    Error {
        code: String,
        message: String,
        data: String,
        /// Present when a generation committed before the failure
        /// (GeneratorFailed): pruning must still run.
        published: Option<PublishedGeneration>,
    },
}

pub struct Daemon {
    state: State,
    locked: bool,
    /// Pruning deferred until after the response is flushed.
    pending_prune: Option<(PathBuf, PublishedGeneration)>,
}

enum State {
    Uninitialized,
    Ready(Box<Session>),
}

struct Session {
    project: Project,
    project_base: PathBuf,
    exclude_roots: Vec<String>,
    diagnostic_level: DiagnosticLevel,
    /// `None` after a failed config/schema reload: every request retries
    /// the full load until it succeeds.
    bowl: Option<Bowl>,
    last: Option<Outcome>,
    locked: bool,
    /// Hash of the last bytes observed or accepted at `dsql/dsql.lock`.
    filter_match_lock_hash: Option<String>,
}

impl Daemon {
    pub fn new(locked: bool) -> Self {
        Self {
            state: State::Uninitialized,
            locked,
            pending_prune: None,
        }
    }

    /// Post-response work: pruning happens after the consumer has its
    /// answer (spec: respond, then maintain).
    pub async fn after_respond(&mut self) {
        if let Some((build_dir, published)) = self.pending_prune.take() {
            let _ = tokio::task::spawn_blocking(move || {
                dsql_generate::publish::prune(&build_dir, &published);
            })
            .await;
        }
    }

    pub async fn handle(&mut self, request: Request) -> Handled {
        match request.method {
            Method::Shutdown => Handled::Shutdown(result_line(request.id, "true")),
            Method::Initialize => Handled::Respond(self.initialize(request).await),
            Method::Compile | Method::FilesChanged => {
                let State::Ready(_) = &self.state else {
                    return Handled::Respond(error_line(
                        Some(request.id),
                        "NotInitialized",
                        "initialize the daemon first",
                        "null",
                    ));
                };
                Handled::Respond(self.compile(request).await)
            }
        }
    }

    async fn initialize(&mut self, request: Request) -> String {
        if let State::Ready(_) = &self.state {
            return error_line(
                Some(request.id),
                "AlreadyInitialized",
                "the daemon is already initialized",
                "null",
            );
        }
        let Some(version) = request.params.protocol_version else {
            return error_line(
                Some(request.id),
                "InvalidRequest",
                "initialize requires params.protocolVersion",
                "{\"method\":\"initialize\"}",
            );
        };
        if version != PROTOCOL_VERSION {
            return error_line(
                Some(request.id),
                "UnsupportedProtocolVersion",
                &format!("daemon speaks protocol {PROTOCOL_VERSION}, consumer sent {version}"),
                &format!("{{\"daemonVersion\":{PROTOCOL_VERSION}}}"),
            );
        }
        let Some(root) = request.params.root.as_deref() else {
            return error_line(
                Some(request.id),
                "InvalidRequest",
                "initialize requires params.root",
                "{\"method\":\"initialize\"}",
            );
        };
        let diagnostic_level =
            match DiagnosticLevel::parse(request.params.diagnostic_level.as_deref()) {
                Ok(level) => level,
                Err(message) => {
                    return error_line(
                        Some(request.id),
                        "InvalidRequest",
                        &message,
                        "{\"method\":\"initialize\"}",
                    );
                }
            };
        let project = match Project::load_from(Path::new(root)).await {
            Ok(project) => project,
            Err(error) => {
                return error_line(
                    Some(request.id),
                    "ProjectLoadFailed",
                    &error.to_string(),
                    &format!("{{\"message\":{}}}", json_string(&error.to_string())),
                );
            }
        };
        // Canonicalize once; every later path exchange is lexical.
        let base = project.base().to_path_buf();
        let project_base = std::fs::canonicalize(&base).unwrap_or(base);
        let mut exclude_roots = Vec::new();
        for root in request.params.exclude_roots.clone().unwrap_or_default() {
            match dsql_project::validate_reserved_root(&project.config, &root) {
                Ok(normalized) => exclude_roots.push(normalized),
                Err(error) => {
                    return error_line(
                        Some(request.id),
                        "InvalidPath",
                        &error.to_string(),
                        &format!("{{\"path\":{}}}", json_string(&root)),
                    );
                }
            }
        }
        let generator_outputs = project.config.generate.typescript.outputs.clone();
        let filter_match_lock_hash = std::fs::read(project.root.join("dsql.lock"))
            .ok()
            .map(|bytes| sha256_hex(&bytes));

        let body = format!(
            "{{\"protocolVersion\":{PROTOCOL_VERSION},\"projectBase\":{},\"configPath\":\"dsql/dsql.toml\",\"schemaDir\":\"dsql/schema\",\"buildDir\":\"dsql/build\",\"generatorOutputs\":{},\"diagnosticLevel\":{}}}",
            json_string(&project_base.to_string_lossy()),
            string_array(&generator_outputs),
            json_string(diagnostic_level.as_str()),
        );
        self.state = State::Ready(Box::new(Session {
            project,
            project_base,
            exclude_roots,
            diagnostic_level,
            bowl: None,
            last: None,
            locked: self.locked,
            filter_match_lock_hash,
        }));
        result_line(request.id, &body)
    }

    async fn compile(&mut self, request: Request) -> String {
        let State::Ready(session) = &mut self.state else {
            unreachable!("guarded by handle()");
        };
        let id = request.id;

        // Classify the batch (filesChanged) or force a full pass (compile).
        let plan = match request.method {
            Method::Compile => BatchPlan::FullReload(None),
            Method::FilesChanged => match session.classify_batch(&request).await {
                Ok(plan) => plan,
                Err(response) => return respond_error_for(id, response),
            },
            _ => unreachable!(),
        };

        if matches!(plan, BatchPlan::NoOp) {
            // Replay the last outcome verbatim; project-load retry is the
            // one exception, handled below because bowl == None.
            if session.bowl.is_some()
                && let Some(outcome) = &session.last
            {
                return render_outcome(id, outcome, true);
            }
        }

        // Apply the plan to the resident bowl (or reload).
        let applied = session.apply(plan).await;
        if let Err(error) = applied {
            session.bowl = None;
            return error_line(
                Some(id),
                "ProjectLoadFailed",
                &error,
                &format!("{{\"message\":{}}}", json_string(&error)),
            );
        }

        let outcome = session.compile_now().await;
        let response = render_outcome(id, &outcome, false);
        // Success AND GeneratorFailed both committed a generation; prune
        // either way once the response is flushed.
        let published = match &outcome {
            Outcome::Success { published, .. } => Some(published.clone()),
            Outcome::Error { published, .. } => published.clone(),
        };
        if let Some(published) = published {
            session.filter_match_lock_hash = published.filter_match_lock_hash.clone();
            self.pending_prune = Some((session.project.root.join("build"), published));
        }
        session.last = Some(outcome);
        response
    }
}

/// How a changed file relates to the project.
enum Relevance {
    Relevant,
    Irrelevant,
    /// Two scopes claim it — reload so the loader reports the ownership
    /// conflict exactly as a cold start would.
    Ambiguous,
}

enum BatchPlan {
    /// Nothing relevant changed.
    NoOp,
    /// Apply these file upserts — the exact bytes classification read —
    /// to the resident bowl. Application never re-reads disk.
    Incremental(Vec<(PathBuf, String)>, Option<Option<String>>),
    /// Only the on-disk match lock changed; the bowl stays resident.
    MatchLockChanged(Option<String>),
    /// Config/schema/deletion or no resident bowl: reload from disk.
    FullReload(Option<Option<String>>),
}

/// How one relevant file's on-disk bytes relate to the resident bowl.
enum FileChange {
    /// The bowl holds these exact bytes: a protocol no-op.
    Unchanged,
    /// New content, carried to application verbatim.
    Fresh(String),
}

fn respond_error_for(id: u64, (code, message, data): (String, String, String)) -> String {
    error_line(Some(id), &code, &message, &data)
}

fn render_outcome(id: u64, outcome: &Outcome, replay: bool) -> String {
    match outcome {
        Outcome::Success { body, changed, .. } => {
            let flag = if *changed && !replay { "true" } else { "false" };
            result_line(id, &body.replace("%CHANGED%", flag))
        }
        Outcome::Error {
            code,
            message,
            data,
            ..
        } => error_line(Some(id), code, message, data),
    }
}

impl Session {
    /// Normalizes and classifies a `filesChanged` batch before any
    /// mutation: outside paths reject, irrelevant paths drop (scope
    /// relevance is the daemon's judgment), config or schema (or
    /// ancestor-directory) changes and deletions force a full reload
    /// (the engine has no external despawn, so removals reload).
    async fn classify_batch(
        &self,
        request: &Request,
    ) -> Result<BatchPlan, (String, String, String)> {
        let Some(paths) = request.params.paths.as_ref() else {
            return Err((
                "InvalidRequest".into(),
                "filesChanged requires params.paths".into(),
                "{\"method\":\"filesChanged\"}".into(),
            ));
        };
        if self.bowl.is_none() {
            return Ok(BatchPlan::FullReload(None));
        }
        let mut upserts = Vec::new();
        let mut reload = false;
        let mut lock_change = None;
        for raw in paths {
            let relative = match self.normalize(raw) {
                Ok(relative) => relative,
                Err(problem) => {
                    return Err((
                        "InvalidPath".into(),
                        problem,
                        format!("{{\"path\":{}}}", json_string(raw)),
                    ));
                }
            };
            let text = relative.to_string_lossy().replace('\\', "/");
            if text == "dsql/dsql.lock" {
                let absolute = self.project_base.join(&relative);
                let observed = match std::fs::read(&absolute) {
                    Ok(bytes) => Some(sha256_hex(&bytes)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        return Err((
                            "Io".into(),
                            format!("failed to read {}: {error}", absolute.display()),
                            format!("{{\"path\":{}}}", json_string(raw)),
                        ));
                    }
                };
                if observed != self.filter_match_lock_hash {
                    lock_change = Some(observed);
                }
                continue;
            }
            if self.is_reserved(&text) {
                continue;
            }
            if text == "dsql/dsql.toml"
                || text.starts_with("dsql/schema")
                || text == "dsql"
                || text.is_empty()
            {
                reload = true;
                continue;
            }
            let absolute = self.project_base.join(&relative);
            if absolute.is_dir() {
                // Directory events: reconcile the subtree. Removals under
                // it force a reload (the engine has no external despawn);
                // otherwise new/changed relevant files upsert.
                if self.bowl_lost_files_under(&absolute).await {
                    reload = true;
                } else {
                    match self.changed_files_under(&absolute).await {
                        Ok((_, true)) => reload = true,
                        Ok((changed, false)) => upserts.extend(changed),
                        Err(problem) => {
                            return Err((
                                "Io".into(),
                                problem,
                                format!("{{\"path\":{}}}", json_string(raw)),
                            ));
                        }
                    }
                }
            } else if absolute.is_file() {
                match self.relevance(&absolute).await {
                    Relevance::Irrelevant => continue,
                    Relevance::Ambiguous => {
                        reload = true;
                        continue;
                    }
                    Relevance::Relevant => {}
                }
                match self.classify_file(&absolute).await {
                    // Same-content events are protocol no-ops.
                    Ok(FileChange::Unchanged) => {}
                    Ok(FileChange::Fresh(content)) => upserts.push((absolute, content)),
                    Err(problem) => {
                        return Err((
                            "Io".into(),
                            problem,
                            format!("{{\"path\":{}}}", json_string(raw)),
                        ));
                    }
                }
            } else {
                // A deleted path only matters if the bowl actually held
                // files there; a vanished README is nobody's business.
                if self.bowl_has_files_under(&absolute).await {
                    reload = true;
                }
            }
        }
        if reload {
            return Ok(BatchPlan::FullReload(lock_change));
        }
        if !upserts.is_empty() {
            return Ok(BatchPlan::Incremental(upserts, lock_change));
        }
        match lock_change {
            Some(hash) => Ok(BatchPlan::MatchLockChanged(hash)),
            None => Ok(BatchPlan::NoOp),
        }
    }

    /// Whether the bowl holds the file itself or anything under it as a
    /// directory prefix.
    async fn bowl_has_files_under(&self, absolute: &Path) -> bool {
        let Some(bowl) = self.bowl.as_ref() else {
            return false;
        };
        let exact = absolute.to_string_lossy().to_string();
        let prefix = format!("{exact}/");
        let rows = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
        rows.collect()
            .into_iter()
            .any(|(_, path)| path.0 == exact || path.0.starts_with(&prefix))
    }

    /// Whether any bowl file under the directory no longer exists on disk.
    async fn bowl_lost_files_under(&self, directory: &Path) -> bool {
        let Some(bowl) = self.bowl.as_ref() else {
            return false;
        };
        let prefix = format!("{}/", directory.to_string_lossy());
        let rows = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
        rows.collect()
            .into_iter()
            .filter(|(_, path)| path.0.starts_with(&prefix))
            .any(|(_, path)| !Path::new(&path.0).is_file())
    }

    /// Relevant files under a directory whose bytes differ from the bowl
    /// (or are new to it), plus whether an ownership ambiguity forces a
    /// full reload. Filesystem failures surface — silently skipping a
    /// subtree would replay a stale success.
    async fn changed_files_under(
        &self,
        directory: &Path,
    ) -> Result<(Vec<(PathBuf, String)>, bool), String> {
        let mut found = Vec::new();
        let mut stack = vec![directory.to_path_buf()];
        while let Some(current) = stack.pop() {
            let entries = std::fs::read_dir(&current)
                .map_err(|error| format!("failed to scan {}: {error}", current.display()))?;
            for entry in entries {
                let entry = entry
                    .map_err(|error| format!("failed to read {}: {error}", current.display()))?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(&self.project_base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if self.is_reserved(&relative) {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    match self.relevance(&path).await {
                        Relevance::Relevant => match self.classify_file(&path).await? {
                            FileChange::Unchanged => {}
                            FileChange::Fresh(content) => found.push((path, content)),
                        },
                        Relevance::Irrelevant => {}
                        // Cold behavior: the loader reports the conflict.
                        Relevance::Ambiguous => return Ok((Vec::new(), true)),
                    }
                }
            }
        }
        Ok((found, false))
    }

    /// Reads a relevant file ONCE and classifies its content against the
    /// resident bowl as the same bytes (protocol no-op) or fresh bytes.
    /// The classified bytes travel with the plan so application never
    /// re-reads and cannot observe different content after classification.
    async fn classify_file(&self, absolute: &Path) -> Result<FileChange, String> {
        let content = std::fs::read_to_string(absolute)
            .map_err(|error| format!("failed to read {}: {error}", absolute.display()))?;
        let hash = dsql_core::source::content_hash(&content);
        let path_text = absolute.to_string_lossy().to_string();
        if let Some(bowl) = self.bowl.as_ref() {
            let rows = bowl
                .scoop::<Query<(Entity, &FilePath, &SourceText)>>()
                .await;
            if rows
                .collect()
                .into_iter()
                .any(|(_, path, text)| path.0 == path_text && text.content_hash() == hash)
            {
                return Ok(FileChange::Unchanged);
            }
        }
        Ok(FileChange::Fresh(content))
    }

    /// A file is relevant when the bowl already carries it or when the
    /// scope configuration owns its path (new files join their scope).
    async fn relevance(&self, absolute: &Path) -> Relevance {
        let Some(bowl) = self.bowl.as_ref() else {
            return Relevance::Relevant;
        };
        let path_text = absolute.to_string_lossy().to_string();
        {
            let rows = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
            if rows
                .collect()
                .into_iter()
                .any(|(_, path)| path.0 == path_text)
            {
                return Relevance::Relevant;
            }
        }
        let scopes = bowl
            .scoop::<Query<(Entity, &dsql_core::source::ScopeDocuments)>>()
            .await;
        let ownership = scopes
            .collect()
            .into_iter()
            .next()
            .map(|(_, documents)| documents.ownership_of(&path_text));
        match ownership {
            Some(dsql_core::source::ScopeOwnership::Unique(_)) => Relevance::Relevant,
            Some(dsql_core::source::ScopeOwnership::ImplicitDefault) | None => {
                Relevance::Irrelevant
            }
            // A newly ambiguous file must behave like a cold reload: the
            // loader reports a duplicate document assignment. Forcing the reload
            // keeps warm and cold behavior identical.
            Some(dsql_core::source::ScopeOwnership::Ambiguous(_)) => Relevance::Ambiguous,
            Some(dsql_core::source::ScopeOwnership::Unmatched) => Relevance::Irrelevant,
        }
    }

    fn normalize(&self, raw: &str) -> Result<PathBuf, String> {
        let path = Path::new(raw);
        let joined = if path.is_absolute() {
            path.strip_prefix(&self.project_base)
                .map_err(|_| format!("{raw} is outside the project"))?
                .to_path_buf()
        } else {
            path.to_path_buf()
        };
        let mut normalized = PathBuf::new();
        for component in joined.components() {
            use std::path::Component;
            match component {
                Component::Normal(part) => normalized.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(format!("{raw} escapes the project"));
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("{raw} is outside the project"));
                }
            }
        }
        Ok(normalized)
    }

    fn is_reserved(&self, relative: &str) -> bool {
        let reserved = self
            .exclude_roots
            .iter()
            .map(String::as_str)
            .chain(
                self.project
                    .config
                    .generate
                    .typescript
                    .outputs
                    .iter()
                    .map(String::as_str),
            )
            .chain(std::iter::once("dsql/build"));
        for root in reserved {
            let root = root.trim_matches('/');
            if relative == root || relative.starts_with(&format!("{root}/")) {
                return true;
            }
        }
        false
    }

    async fn apply(&mut self, plan: BatchPlan) -> Result<(), String> {
        match plan {
            BatchPlan::NoOp => Ok(()),
            BatchPlan::MatchLockChanged(hash) => {
                self.filter_match_lock_hash = hash;
                Ok(())
            }
            BatchPlan::Incremental(paths, lock_hash) => {
                if let Some(lock_hash) = lock_hash {
                    self.filter_match_lock_hash = lock_hash;
                }
                let bowl = self.bowl.as_ref().expect("classified against a bowl");
                for (absolute, content) in paths {
                    upsert(bowl, &absolute, &content).await;
                }
                Ok(())
            }
            BatchPlan::FullReload(lock_hash) => {
                if let Some(lock_hash) = lock_hash {
                    self.filter_match_lock_hash = lock_hash;
                }
                // Re-read config + schema too: a transparent full reload.
                let project = Project::load_from(&self.project_base)
                    .await
                    .map_err(|error| error.to_string())?;
                // Consumer exclusions must stay valid against the NEW
                // config (a reload can change scopes and outputs).
                for root in &self.exclude_roots {
                    dsql_project::validate_reserved_root(&project.config, root)
                        .map_err(|error| error.to_string())?;
                }
                let bowl = dsql_core::language_bowl().await;
                dsql_project::populate_project_bowl_excluding(&bowl, &project, &self.exclude_roots)
                    .await
                    .map_err(|error| error.to_string())?;
                dsql_core::facts::arm_generate_demands(&bowl).await;
                self.project = project;
                self.bowl = Some(bowl);
                Ok(())
            }
        }
    }

    /// The compile proper: diagnostics snapshot, assembly, publication,
    /// generator, response body.
    async fn compile_now(&mut self) -> Outcome {
        let bowl = self.bowl.as_ref().expect("apply() left a bowl");
        let mut diagnostics = collect_diagnostics(bowl, &self.project_base).await;
        // Policy diagnostics join the snapshot BEFORE the error gate and
        // the normative sort, so they appear in every response.
        if let Some(warning) = self.generator_policy_warning() {
            diagnostics.push(warning);
            diagnostics.sort_by(|left, right| {
                (&left.file, left.start, &left.code).cmp(&(&right.file, right.start, &right.code))
            });
        }
        let errors = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == "Error")
            .count();
        let visible_diagnostics = diagnostics
            .iter()
            .filter(|diagnostic| self.diagnostic_level.includes(&diagnostic.severity))
            .cloned()
            .collect::<Vec<_>>();
        if errors > 0 {
            return Outcome::Error {
                code: "Diagnostics".into(),
                message: "cannot generate while diagnostics contain errors".into(),
                data: format!(
                    "{{\"diagnostics\":{}}}",
                    render_diagnostics(&visible_diagnostics)
                ),
                published: None,
            };
        }

        let assembled =
            match dsql_generate::assemble_project(bowl, &self.project, GenerateOptions::default())
                .await
            {
                Ok(assembled) => assembled,
                Err(error) => return generate_outcome_error(error),
            };
        let callsites = match collect_callsites(bowl, &self.project_base).await {
            Ok(callsites) => callsites,
            Err(message) => {
                return Outcome::Error {
                    code: "Internal".into(),
                    message,
                    data: "null".into(),
                    published: None,
                };
            }
        };
        let scopes = collect_source_scopes(bowl, &self.project_base).await;
        let published = match dsql_generate::publish_snapshot(
            &self.project,
            &assembled.snapshot,
            if self.locked {
                MatchLockMode::Locked
            } else {
                MatchLockMode::Update
            },
        )
        .await
        {
            Ok(published) => published,
            Err(error) => return generate_outcome_error(error),
        };

        let generator = self.run_generator(&published).await;
        if let Err(status) = generator {
            // The generation COMMITTED before the generator ran: report
            // it in data, and pruning still happens (via `published`).
            return Outcome::Error {
                code: "GeneratorFailed".into(),
                message: status,
                data: format!(
                    "{{\"generationId\":{},\"manifestPath\":{}}}",
                    published.generation_id,
                    json_string(&relative_to(&self.project_base, &published.manifest_path)),
                ),
                published: Some(published),
            };
        }

        let manifest = published.manifest_json.clone();

        let artifacts: Vec<String> = assembled
            .snapshot
            .artifacts
            .iter()
            .map(|artifact| {
                format!(
                    "{{\"id\":{},\"kind\":{},\"scope\":{},\"metadata\":{}}}",
                    json_string(&artifact.id),
                    json_string(artifact.family.label()),
                    json_string(&artifact.scope),
                    artifact.serialized,
                )
            })
            .collect();
        let groups: Vec<String> = assembled
            .snapshot
            .groups
            .iter()
            .map(|group| {
                format!(
                    "{{\"name\":{},\"imports\":{},\"generationTarget\":{},\"artifacts\":{}}}",
                    json_string(&group.name),
                    string_array(&group.imports),
                    group.generation_target,
                    string_array(&group.artifacts),
                )
            })
            .collect();
        let contract = &assembled.snapshot.project_contract.fingerprint;

        let body = format!(
            "{{\"generationId\":{},\"changed\":%CHANGED%,\"manifestPath\":{},\"currentManifestPath\":{},\"projectContractHash\":{{\"algorithm\":{},\"value\":{}}},\"manifest\":{},\"artifacts\":[{}],\"groups\":[{}],\"sourceFileScopes\":{scopes},\"callsites\":{callsites},\"diagnostics\":{}}}",
            published.generation_id,
            json_string(&relative_to(&self.project_base, &published.manifest_path)),
            json_string(&relative_to(
                &self.project_base,
                &published.current_manifest_path
            )),
            json_string(&contract.algorithm),
            json_string(&contract.value),
            manifest,
            artifacts.join(","),
            groups.join(","),
            render_diagnostics(&visible_diagnostics),
        );
        Outcome::Success {
            body,
            changed: !published.written.is_empty(),
            published,
        }
    }

    /// The host generator under daemon policy: enabled-but-undeclared
    /// outputs skip with a warning diagnostic (an unexcludable generator
    /// inside a watch loop is an infinite cycle).
    /// The skip warning, computed BEFORE the compile's error gate so it
    /// joins every diagnostics snapshot in sorted position.
    fn generator_policy_warning(&self) -> Option<WireDiagnostic> {
        let typescript = &self.project.config.generate.typescript;
        let skipped =
            typescript.enabled && !typescript.cmd.is_empty() && typescript.outputs.is_empty();
        skipped.then(|| WireDiagnostic {
            file: "dsql/dsql.toml".to_string(),
            start: 0,
            end: 0,
            embedded: None,
            severity: "Warning".to_string(),
            source: "Generate".to_string(),
            code: "GeneratorSkipped".to_string(),
            message: "generator command skipped: declare [generate.typescript] outputs so \
                      daemon consumers can exclude them from watching"
                .to_string(),
        })
    }

    async fn run_generator(&self, published: &PublishedGeneration) -> Result<(), String> {
        let typescript = &self.project.config.generate.typescript;
        if !typescript.enabled || typescript.cmd.is_empty() || typescript.outputs.is_empty() {
            // The undeclared-outputs skip already warned via
            // generator_policy_warning.
            return Ok(());
        }
        let status = tokio::process::Command::new(&typescript.cmd[0])
            .args(&typescript.cmd[1..])
            .env("DSQL_PROJECT_DIR", &self.project_base)
            .env("DSQL_MANIFEST", &published.manifest_path)
            .current_dir(&self.project_base)
            // The daemon's stdio IS the protocol: a generator must never
            // read consumer requests or print into the response stream.
            // stderr stays inherited for logging.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .await
            .map_err(|error| format!("failed to spawn host generator: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("host generator failed with {status}"))
        }
    }
}

fn generate_outcome_error(error: GenerateError) -> Outcome {
    let (code, data) = match &error {
        GenerateError::ArtifactCollision(collision) => (
            "ArtifactCollision",
            format!(
                "{{\"kind\":{},\"first\":{},\"firstSource\":{},\"second\":{},\"secondSource\":{},\"path\":{}}}",
                json_string(collision.kind),
                json_string(&collision.first),
                json_string(&collision.first_source),
                json_string(&collision.second),
                json_string(&collision.second_source),
                json_string(&collision.path),
            ),
        ),
        GenerateError::PublicationLocked => ("PublicationLocked", "null".to_string()),
        GenerateError::Assembly { name, message } | GenerateError::Serialize { name, message } => (
            "AssemblyFailed",
            format!(
                "{{\"artifact\":{},\"message\":{}}}",
                json_string(name),
                json_string(message),
            ),
        ),
        GenerateError::MatchLock { .. } => {
            let diagnostic = WireDiagnostic {
                file: "dsql/dsql.lock".to_string(),
                start: 0,
                end: 0,
                embedded: None,
                severity: "Error".to_string(),
                source: "dsql".to_string(),
                code: "FilterMatchLock".to_string(),
                message: error.to_string(),
            };
            (
                "Diagnostics",
                format!("{{\"diagnostics\":{}}}", render_diagnostics(&[diagnostic])),
            )
        }
        GenerateError::LanguageDiagnostics { .. } => (
            // collect_diagnostics answered first in the normal path; this
            // arm covers races where new errors appeared mid-assembly.
            "Diagnostics",
            "{\"diagnostics\":[]}".to_string(),
        ),
        // The protocol specifies data: null for Internal; the message
        // already rides the human-readable field.
        GenerateError::Internal(_) => ("Internal", "null".to_string()),
        GenerateError::Project(project) => (
            "ProjectLoadFailed",
            format!("{{\"message\":{}}}", json_string(&project.to_string())),
        ),
        GenerateError::AddressCollision { path, id } => (
            "Io",
            format!(
                "{{\"path\":{},\"artifact\":{}}}",
                json_string(path),
                json_string(id),
            ),
        ),
        GenerateError::Write { path, .. } | GenerateError::Io { path, .. } => (
            "Io",
            format!("{{\"path\":{}}}", json_string(&path.to_string_lossy())),
        ),
        _ => ("Io", "null".to_string()),
    };
    Outcome::Error {
        code: code.to_string(),
        message: error.to_string(),
        data,
        published: None,
    }
}

async fn upsert(bowl: &Bowl, absolute: &Path, content: &str) {
    let path_text = absolute.to_string_lossy().to_string();
    let target = {
        let rows = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
        rows.collect()
            .into_iter()
            .find(|(_, path)| path.0 == path_text)
            .map(|(entity, _)| entity)
    };
    let found = target.is_some();
    if let Some(target) = target {
        let rows = bowl.scoop::<Query<(Entity, Mut<SourceText>)>>().await;
        for (entity, text) in rows.collect() {
            if entity == target {
                let content = content.to_string();
                text.with_latest(move |text| text.set_text(&content)).await;
            }
        }
    }
    if !found {
        // A new file joins its configured scope, resolved from the bowl's
        // own ScopeDocuments — never a whole-project rediscovery whose
        // unrelated I/O failures could silently drop this file.
        let scopes = bowl
            .scoop::<Query<(Entity, &dsql_core::source::ScopeDocuments)>>()
            .await;
        let ownership = scopes
            .collect()
            .into_iter()
            .next()
            .map(|(_, documents)| documents.ownership_of(&path_text));
        use dsql_core::source::ScopeOwnership;
        let assignment = match ownership {
            Some(ScopeOwnership::Unique(assignment)) => Some(assignment),
            _ => None,
        };
        if let Some(assignment) = assignment {
            insert_source_scoped(
                bowl,
                &path_text,
                content,
                ResolutionScope(assignment.scope),
                assignment.kind,
            )
            .await;
        }
    }
}

fn relative_to(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn string_array(values: &[String]) -> String {
    let rendered: Vec<String> = values.iter().map(|value| json_string(value)).collect();
    format!("[{}]", rendered.join(","))
}

/// The complete diagnostics snapshot: every fact, host coordinates,
/// embedded ranges for region findings, deterministic order.
async fn collect_diagnostics(bowl: &Bowl, base: &Path) -> Vec<WireDiagnostic> {
    type Row<'a> = (
        Entity,
        &'a Severity,
        &'a Span,
        &'a Diagnostic,
        &'a DiagnosticCode,
        &'a DiagnosticSource,
        &'a BelongsToFile,
    );
    let rows = bowl.scoop::<Query<Row<'_>>>().await;
    let paths = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let paths = paths.collect();
    let regions = bowl
        .scoop::<Query<(Entity, &BelongsToHost, &SourceOffset)>>()
        .await;
    let regions = regions.collect();

    let locate = |file: Entity| -> Option<(String, usize, bool)> {
        let region = regions.iter().find(|(entity, _, _)| *entity == file);
        let (target, offset, embedded) = region.map_or((file, 0, false), |(_, host, offset)| {
            (host.0, offset.0, true)
        });
        let (_, path) = paths.iter().find(|(entity, _)| *entity == target)?;
        let relative = relative_to(base, Path::new(&path.0));
        Some((relative, offset, embedded))
    };

    let mut diagnostics: Vec<WireDiagnostic> = rows
        .collect()
        .into_iter()
        .filter_map(|(_, severity, span, diagnostic, code, source, file)| {
            let (path, offset, embedded) = locate(file.0)?;
            Some(WireDiagnostic {
                file: path,
                start: offset + span.start,
                end: offset + span.end,
                embedded: embedded.then_some((span.start, span.end)),
                severity: format!("{severity:?}"),
                source: format!("{source:?}"),
                code: format!("{code:?}"),
                message: diagnostic.0.clone(),
            })
        })
        .collect();
    diagnostics.sort_by(|left, right| {
        (&left.file, left.start, &left.code).cmp(&(&right.file, right.start, &right.code))
    });
    diagnostics
}

/// Callsites grouped per host file, with one semantically resolved artifact
/// target per expression (docs/spec/build-daemon.md, Compile result).
async fn collect_callsites(bowl: &Bowl, base: &Path) -> Result<String, String> {
    let regions = bowl
        .scoop::<Query<(Entity, &BelongsToHost, &CallsiteSpan)>>()
        .await;
    let regions = regions.collect();
    let paths = bowl.scoop::<Query<(Entity, &FilePath)>>().await;
    let paths = paths.collect();
    let texts = bowl.scoop::<Query<(Entity, &SourceText)>>().await;
    let texts = texts.collect();
    let resolvers = bowl.scoop::<Query<(Entity, &ExtractionResolver)>>().await;
    let resolvers = resolvers.collect();
    let resolved = bowl
        .scoop::<Query<(Entity, &ResolvedEmbeddedExpression, &BelongsToFile)>>()
        .await;
    let resolved = resolved.collect();
    let defs = bowl
        .scoop::<Query<(Entity, &DefDecl, &ResolutionScope)>>()
        .await;
    let defs = defs.collect();

    type HostExpressions = (Entity, String, Vec<(usize, usize, Entity)>);
    let mut hosts: std::collections::BTreeMap<String, HostExpressions> =
        std::collections::BTreeMap::new();
    for (region, host, callsite) in &regions {
        let Some((_, path)) = paths.iter().find(|(entity, _)| entity == &host.0) else {
            continue;
        };
        let relative = relative_to(base, Path::new(&path.0));
        let Some((_, resolver)) = resolvers.iter().find(|(entity, _)| entity == &host.0) else {
            return Err(format!(
                "embedding host `{relative}` has no extraction resolver"
            ));
        };
        hosts
            .entry(relative)
            .or_insert_with(|| (host.0, resolver.0.clone(), Vec::new()))
            .2
            .push((callsite.0.start, callsite.0.end, *region));
    }

    let mut rendered = Vec::new();
    for (path, (host, resolver, mut expressions)) in hosts {
        expressions.sort();
        let content_hash = texts
            .iter()
            .find(|(entity, _)| entity == &host)
            .and_then(|(_, text)| text.to_text())
            .map(|text| sha256_hex(text.as_bytes()))
            .unwrap_or_default();
        let mut expression_json = Vec::new();
        for (start, end, region) in expressions {
            let Some((_, resolution, _)) = resolved.iter().find(|(_, _, file)| file.0 == region)
            else {
                return Err(format!(
                    "embedded expression `{path}` at {start}..{end} has no semantic target fact"
                ));
            };
            let EmbeddedExpressionResolution::Target(target) = resolution.0 else {
                return Err(format!(
                    "embedded expression `{path}` at {start}..{end} has no rewrite target after diagnostics passed"
                ));
            };
            let Some((_, decl, scope)) =
                defs.iter().find(|(definition, _, _)| definition == &target)
            else {
                return Err(format!(
                    "embedded expression `{path}` at {start}..{end} targets a missing definition"
                ));
            };
            let family = match decl.kind {
                DefKind::Query => "operation",
                DefKind::Fragment => "fragment",
            };
            let target = format!("{}/{family}/{}", scope.0, decl.name);
            expression_json.push(format!(
                "{{\"range\":{{\"start\":{start},\"end\":{end}}},\"target\":{}}}",
                json_string(&target),
            ));
        }
        rendered.push(format!(
            "{{\"path\":{},\"resolver\":{},\"contentHash\":{{\"algorithm\":\"sha256\",\"value\":{}}},\"expressions\":[{}]}}",
            json_string(&path),
            json_string(&resolver),
            json_string(&content_hash),
            expression_json.join(","),
        ));
    }
    Ok(format!("[{}]", rendered.join(",")))
}

/// Which scope owns each source file (informational, per the spec).
async fn collect_source_scopes(bowl: &Bowl, base: &Path) -> String {
    let paths = bowl
        .scoop::<Query<(Entity, &FilePath, &ResolutionScope)>>()
        .await;
    let regions = bowl.scoop::<Query<(Entity, &BelongsToHost)>>().await;
    let region_entities: Vec<Entity> = regions
        .collect()
        .into_iter()
        .map(|(entity, _)| entity)
        .collect();
    let mut rows: Vec<(String, String)> = paths
        .collect()
        .into_iter()
        .filter(|(entity, _, _)| !region_entities.contains(entity))
        .map(|(_, path, scope)| (relative_to(base, Path::new(&path.0)), scope.0.clone()))
        .collect();
    rows.sort();
    let rendered: Vec<String> = rows
        .into_iter()
        .map(|(path, scope)| {
            format!(
                "{{\"path\":{},\"scope\":{}}}",
                json_string(&path),
                json_string(&scope),
            )
        })
        .collect();
    format!("[{}]", rendered.join(","))
}
