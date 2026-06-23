pub(crate) use crate::{
    format::cst::CstFormatter,
    language::{
        atom::language_atom,
        stages::{
            BuildsAst, Checker, ChecksAtom, EditorFeatures, FormatsAtom, GeneratesMetadataAtom,
            InfersVariablesAtom, Lowerer, LowersAtom, MetadataGenerator, Planner, PlansAtom,
            ProvidesEditorSupport, VariableInference,
        },
    },
    semantic::{Interner, NameId, NameIndex},
    syntax::{
        NameRef, TextRange,
        grammar::parser::{NodeRef, Rule},
        parse::AstBuilder,
    },
};
