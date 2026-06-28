use dsql_core::{
    LanguageAtoms, LanguageContext, LanguageContextProvider, LanguageServiceRequest,
    RuleClassification, SourceSnapshot, SyntaxRule, TextRange, parse_source,
};
use insta::Settings;
use std::path::PathBuf;

#[test]
fn directive_language_contexts_are_ranked() {
    let cases = [
        (
            "namespace",
            "query DirectiveContexts @ { users { id } }",
            "query DirectiveContexts @",
        ),
        (
            "shorthand_member",
            "query DirectiveContexts { users @. { id } }",
            "users @.",
        ),
        (
            "explicit_member",
            "query DirectiveContexts { users @dsql. { id } }",
            "users @dsql.",
        ),
        (
            "argument",
            "query DirectiveContexts { users { id @.include_if( } }",
            "@.include_if(",
        ),
        (
            "boolean_value",
            "query DirectiveContexts { users { id @.include_if(if:  } }",
            "@.include_if(if: ",
        ),
    ];

    let output = cases
        .iter()
        .map(|(name, source, marker)| {
            let parsed = parse_source(SourceSnapshot::from(*source));
            let byte = source.find(marker).expect("marker should exist") + marker.len();
            let contexts = LanguageContextProvider::contexts(LanguageServiceRequest {
                parse: &parsed,
                byte,
            });
            format!(
                "{name}\n{}",
                contexts
                    .iter()
                    .map(|context| format_context(context))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshots"));
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        insta::assert_snapshot!("language_context__directives", output);
    });
}

fn format_context(context: &LanguageContext<'_>) -> String {
    let rule = format_rule(context.rule);
    let classification = format_classification(LanguageAtoms::classify(context.rule.into()));
    let token = context
        .token
        .map_or_else(|| "-".to_string(), |token| format_token(context, token));
    let construct = format_range(context.construct_range);
    let focus = format_range(context.focus_range);

    format!(
        "{:?} {:?} rule={rule} class={classification} token={token} construct={construct} focus={focus}",
        context.confidence, context.origin
    )
}

fn format_rule(rule: SyntaxRule) -> String {
    format!("{rule:?}")
}

fn format_token(context: &LanguageContext<'_>, node: usize) -> String {
    let syntax_node = &context.request.parse.tree.nodes[node];
    let text = context.request.parse.source.text(TextRange::new(
        syntax_node.range.start as usize,
        syntax_node.range.end as usize,
    ));
    match syntax_node.cst_kind {
        dsql_core::CstKind::Token(token) => format!("{token:?}({:?})", text.as_ref()),
        dsql_core::CstKind::Rule(_) => "-".to_string(),
    }
}

fn format_range(range: TextRange) -> String {
    format!("{}..{}", range.start, range.end)
}

fn format_classification(classification: RuleClassification) -> String {
    match classification {
        RuleClassification::Owned(atom) => format!("owned:{}", atom.ast),
        RuleClassification::Delegated(atom) => format!("delegated:{}", atom.ast),
    }
}
