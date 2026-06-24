use crate::language::{
    grammar::LanguageAtoms,
    stages::{EditorCompletion, EditorCompletionRequest},
};

/// Runs editor completion providers owned by language atoms.
///
/// This dispatcher is intentionally generic over atom support: frontend callers
/// provide cursor-local source context, and each atom decides whether that
/// context applies to its syntax.
pub fn editor_completions(request: EditorCompletionRequest<'_>) -> Vec<EditorCompletion> {
    let mut completions = Vec::new();
    for completer in LanguageAtoms::completers() {
        completions.extend(completer.completions(request));
    }
    completions
}
