use crate::AssetRegistry;
use crate::language::{
    context::{
        ContextConfidence, LanguageContextProvider, LanguageServiceAssetContext,
        LanguageServiceContext, LanguageServiceRequest,
    },
    grammar::LanguageAtoms,
    stages::EditorCompletion,
};

/// Runs editor completion providers owned by language atoms.
///
/// The dispatcher owns context ranking and deduplication. Atom providers only
/// decide whether one normalized context can produce completions. This is the
/// reference consumption pattern for language-service features: build generic
/// contexts once, loop registered providers, and avoid branching on concrete
/// atom names in the caller.
pub fn editor_completions(request: LanguageServiceRequest<'_>) -> Vec<EditorCompletion> {
    let mut assets = AssetRegistry::default();
    let asset_context = LanguageServiceAssetContext { request };
    for provider in LanguageAtoms::project_asset_providers() {
        provider.provide(&mut assets.project, &asset_context);
    }

    let contexts = LanguageContextProvider::contexts(request);
    let mut ranked = contexts
        .iter()
        .enumerate()
        .flat_map(|(context_index, context)| {
            let service_context = LanguageServiceContext {
                request,
                language_context: context,
                assets: &assets,
            };
            LanguageAtoms::completers()
                .iter()
                .flat_map(move |completer| {
                    completer
                        .completions(&service_context)
                        .into_iter()
                        .map(move |completion| RankedCompletion {
                            completion,
                            confidence: context.confidence,
                            context_index,
                        })
                })
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .confidence
            .cmp(&left.confidence)
            .then_with(|| left.context_index.cmp(&right.context_index))
    });

    let mut completions = Vec::new();
    for ranked_completion in ranked {
        if !completions
            .iter()
            .any(|completion| same_completion(completion, &ranked_completion.completion))
        {
            completions.push(ranked_completion.completion);
        }
    }
    completions
}

struct RankedCompletion {
    completion: EditorCompletion,
    confidence: ContextConfidence,
    context_index: usize,
}

fn same_completion(left: &EditorCompletion, right: &EditorCompletion) -> bool {
    let EditorCompletion {
        label: left_label,
        kind: left_kind,
        insert_text: left_insert_text,
        detail: _,
    } = left;
    let EditorCompletion {
        label: right_label,
        kind: right_kind,
        insert_text: right_insert_text,
        detail: _,
    } = right;
    left_kind == right_kind && left_label == right_label && left_insert_text == right_insert_text
}
