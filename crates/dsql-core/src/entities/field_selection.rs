//! Field-selection entity: one selected field or nested relation inside a
//! selection set, with its alias and relation-path selector.

use bowl::{Bowl, Commands, Component, DerivedFrom};

use crate::entities::{direct_rule, direct_token, node_span, text};
use crate::entity::{LanguageEntity, LowerCtx, LowerStage};
use crate::facts::{BelongsToFile, NodeKey, ParentKey, Span};
use crate::grammar::lexer::Token;
use crate::grammar::parser::{NodeRef, Rule};

/// One field selection, lowered from `field_selection`. Together with
/// [`ParentKey`] these facts are the flat encoding of the selection tree;
/// sibling order is byte order of [`FieldSel::span`].
#[derive(Component, Debug, Hash)]
#[component(hash)]
pub struct FieldSel {
    /// Output alias, when written as `alias: field`.
    pub alias: Option<String>,
    /// The selected field or relation name (full qualified-name text).
    pub name: String,
    /// Explicit relation column selector, when written as `name->column`.
    pub relation_path: Option<String>,
    /// Span of the selected name (the target, not the alias).
    pub name_span: Span,
    /// Span of the whole selection including its clauses and children.
    pub span: Span,
    /// Whether the selection has a nested selection set. Scalar/relation
    /// mismatches against the catalog are checked in phase 6.
    pub nested: bool,
}

/// Owns `field_selection` (and consumes `field_selection_tail` and
/// `field_suffix` from it).
pub struct FieldSelection;

impl LanguageEntity for FieldSelection {
    const NAME: &'static str = "field_selection";

    async fn register(_db: &Bowl) {
        // Catalog checks over field selections land in phase 6.
    }
}

impl LowerStage for FieldSelection {
    fn lower(ctx: &LowerCtx<'_>, node: NodeRef, commands: &mut Commands) {
        let Some(first_ref) = direct_rule(ctx.cst, node, Rule::RelationRef) else {
            // Error recovery consumed the name; parse diagnostics cover it.
            return;
        };
        let tail = direct_rule(ctx.cst, node, Rule::FieldSelectionTail);
        let tail_ref = tail.and_then(|tail| direct_rule(ctx.cst, tail, Rule::RelationRef));

        // `alias: target` puts the alias first and the target in the tail;
        // without an alias the first relation_ref is the target itself.
        let (alias, target) = match tail_ref {
            Some(target) => {
                let alias_span = node_span(ctx.cst, first_ref);
                (Some(text(ctx.source, alias_span).to_string()), target)
            }
            None => (None, first_ref),
        };

        let Some(name_node) = direct_rule(ctx.cst, target, Rule::QualifiedName) else {
            return;
        };
        let name_span = node_span(ctx.cst, name_node);
        // The `->column` selector Name is a direct child of relation_ref;
        // the relation name's own tokens sit nested inside qualified_name.
        let relation_path =
            direct_token(ctx.cst, target, Token::Name).map(|span| text(ctx.source, span).to_string());

        let suffix = tail.and_then(|tail| direct_rule(ctx.cst, tail, Rule::FieldSuffix));
        let nested = suffix
            .map(|suffix| direct_rule(ctx.cst, suffix, Rule::SelectionSet).is_some())
            .unwrap_or(false);

        let selection = FieldSel {
            alias,
            name: text(ctx.source, name_span).to_string(),
            relation_path,
            name_span,
            span: node_span(ctx.cst, node),
            nested,
        };

        let key = NodeKey {
            file: ctx.file,
            node: node.0,
        };

        match ctx.parent {
            Some(parent) => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                ParentKey(parent),
                selection,
            )),
            // Grammar-wise unreachable (selections only appear inside
            // definitions), but error recovery may orphan one; lower it
            // without a tree position rather than dropping the fact.
            None => commands.insert((
                DerivedFrom::new(ctx.file),
                BelongsToFile(ctx.file),
                key,
                selection,
            )),
        };
    }
}
